use crate::model::ModelChoice;
use crate::process::{self, ManagedChild};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const FM: &str = "/usr/bin/fm";
const GENERATION_TIMEOUT: Duration = Duration::from_secs(300);
/// How many recent exchanges the model sees when a chat resumes. Apple's on-device model
/// has a small context window (about 4k tokens), and long histories make it drift or start
/// copying an earlier reply. Six exchanges keep normal follow-ups working while leaving
/// room for a long answer. The full conversation stays saved and visible.
pub const CONTEXT_TURNS: usize = 6;
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);
const TITLE_MAX_CHARS: usize = 48;
const TITLE_CONTEXT_CHARS: usize = 1500;
const TITLE_INSTRUCTIONS: &str =
    "Write a short title of 3 to 6 words that summarizes the topic of the conversation. \
     Reply with only the title. No quotes, no Markdown, no ending punctuation.";

#[derive(Debug)]
pub enum Event {
    Chunk(String),
    Finished { text: String, transcript: Value },
    Failed(String),
    Cancelled,
}

pub struct Generation {
    pub events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Generation {
    pub fn start_with_program(
        program: PathBuf,
        model: ModelChoice,
        prompt: String,
        transcript: Option<Value>,
        directory: PathBuf,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (sender, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let worker = thread::spawn(move || {
            let result = run(
                &program, model, &prompt, transcript, &directory, &cancel, &sender, &wake,
            );
            let event = match result {
                Ok(Some((text, transcript))) => Event::Finished { text, transcript },
                Ok(None) => Event::Cancelled,
                Err(error) => Event::Failed(error),
            };
            let _ = sender.send(event);
            wake();
        });
        Self {
            events,
            cancelled,
            worker: Some(worker),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("MacAi: the model worker unexpectedly panicked.");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    program: &Path,
    model: ModelChoice,
    prompt: &str,
    transcript: Option<Value>,
    directory: &Path,
    cancelled: &AtomicBool,
    sender: &Sender<Event>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> Result<Option<(String, Value)>, String> {
    let temporary = tempfile::Builder::new()
        .prefix(".generation-")
        .tempdir_in(directory)
        .map_err(|error| format!("Cannot create a private working folder: {error}"))?;
    let output = temporary.path().join("response.json");
    // Like `fm respond` in Terminal: no extra instructions limit the model.
    let mut command = Command::new(program);
    command
        .args(["respond", "--model", model.fm_name(), "--stream"])
        .arg("--save-transcript")
        .arg(&output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let sent = transcript
        .as_ref()
        .map(|full| recent_context(full, CONTEXT_TURNS));
    let previous = transcript;
    if let Some(context) = &sent {
        let input = temporary.path().join("context.json");
        let bytes = serde_json::to_vec(context)
            .map_err(|error| format!("Cannot encode conversation context: {error}"))?;
        fs::write(&input, bytes)
            .map_err(|error| format!("Cannot prepare conversation context: {error}"))?;
        command.arg("--resume").arg(input);
    }
    if cancelled.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let mut child = ManagedChild(command.spawn().map_err(|error| {
        format!(
            "Cannot start /usr/bin/fm: {error}. MacAi requires macOS 27 with Apple Intelligence."
        )
    })?);
    let mut stdin = child
        .0
        .stdin
        .take()
        .ok_or("The model input pipe is missing.")?;
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or("The model output pipe is missing.")?;
    let stderr = child
        .0
        .stderr
        .take()
        .ok_or("The model error pipe is missing.")?;

    let sender = sender.clone();
    let wake = wake.clone();
    let reader = thread::spawn(move || read_stream(stdout, &sender, &wake));
    let error_reader = thread::spawn(move || read_diagnostics(stderr));
    let prompt = prompt.to_owned();
    let writer = thread::spawn(move || stdin.write_all(prompt.as_bytes()));
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::SeqCst) {
            child
                .0
                .kill()
                .map_err(|error| format!("Could not stop the model: {error}"))?;
            child
                .0
                .wait()
                .map_err(|error| format!("Could not reap the model: {error}"))?;
            break None;
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => return Err(format!("Cannot read the model process status: {error}")),
        }
        if started.elapsed() > GENERATION_TIMEOUT {
            return Err("The model took more than five minutes. It has been stopped; please retry with a shorter prompt.".into());
        }
        thread::sleep(Duration::from_millis(25));
    };
    let input_result = writer
        .join()
        .map_err(|_| "The model input worker stopped unexpectedly.")?;
    let text = reader
        .join()
        .map_err(|_| "The model output worker stopped unexpectedly.")?;
    let diagnostics = error_reader
        .join()
        .map_err(|_| "The model diagnostics worker stopped unexpectedly.")?
        .map_err(|error| format!("Cannot read model diagnostics: {error}"))?;
    let Some(status) = status else {
        return Ok(None);
    };
    if !status.success() {
        return Err(explain_failure(model, &diagnostics, &status.to_string()));
    }
    input_result.map_err(|error| format!("Cannot send your message to the model: {error}"))?;
    let text = text?.trim_end().to_owned();
    if text.is_empty() {
        return Err(
            "The model returned an empty reply. Try again or start a new conversation.".into(),
        );
    }
    let bytes = fs::read(&output).map_err(|error| {
        format!("The reply arrived, but its conversation context could not be saved: {error}")
    })?;
    let transcript: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!("The model returned an unreadable conversation transcript: {error}")
    })?;
    let model_name = transcript.get("modelName").and_then(Value::as_str);
    let expected_model = match model {
        ModelChoice::OnDevice => model_name == Some("system"),
        // Apple may label cloud transcripts with a more specific model name.
        ModelChoice::Cloud => model_name.is_some_and(|name| !name.is_empty()),
    };
    if transcript.get("transcript").is_none() || !expected_model {
        return Err("The model returned an unexpected transcript format. Your previous conversation context is unchanged.".into());
    }
    let saved = match (&previous, &sent) {
        (Some(full), Some(sent)) => merge_context(full, sent, transcript),
        _ => transcript,
    };
    Ok(Some((text, saved)))
}

fn entries(transcript: &Value) -> Option<&Vec<Value>> {
    transcript
        .pointer("/transcript/transcript/entries")
        .and_then(Value::as_array)
}

/// A copy of `transcript` with only the last `turns` exchanges. Entries before the first
/// user prompt (such as instructions) are always kept. Unknown formats pass through.
pub fn recent_context(transcript: &Value, turns: usize) -> Value {
    let Some(list) = entries(transcript) else {
        return transcript.clone();
    };
    let prompts: Vec<usize> = list
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.get("role").and_then(Value::as_str) == Some("user"))
        .map(|(index, _)| index)
        .collect();
    if prompts.len() <= turns {
        return transcript.clone();
    }
    let preamble = prompts[0];
    let start = prompts[prompts.len() - turns];
    let kept: Vec<Value> = list[..preamble]
        .iter()
        .chain(&list[start..])
        .cloned()
        .collect();
    let mut trimmed = transcript.clone();
    trimmed["transcript"]["transcript"]["entries"] = Value::Array(kept);
    trimmed
}

/// Adds the entries `fm` appended to the trimmed context back onto the full saved history,
/// so trimming never loses older context from disk.
pub fn merge_context(full: &Value, sent: &Value, returned: Value) -> Value {
    let (Some(full_entries), Some(sent_entries), Some(returned_entries)) =
        (entries(full), entries(sent), entries(&returned))
    else {
        return returned;
    };
    let id = |entry: &Value| entry.get("id").cloned();
    let unchanged = returned_entries.len() >= sent_entries.len()
        && sent_entries
            .iter()
            .zip(returned_entries)
            .all(|(a, b)| id(a).is_some() && id(a) == id(b));
    if !unchanged {
        return returned;
    }
    let mut merged = returned.clone();
    let mut all = full_entries.clone();
    all.extend(returned_entries[sent_entries.len()..].iter().cloned());
    merged["transcript"]["transcript"]["entries"] = Value::Array(all);
    merged
}

/// Writes a short title for a conversation with the on-device model. This is a
/// separate, one-off `fm` call: it never resumes or changes the chat's transcript.
pub struct TitleTask {
    pub chat_id: String,
    pub events: Receiver<Result<String, String>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl TitleTask {
    pub fn start(
        chat_id: String,
        prompt: &str,
        reply: &str,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        Self::start_with_program(PathBuf::from(FM), chat_id, prompt, reply, wake)
    }

    #[cfg(test)]
    pub fn finished(chat_id: String, events: Receiver<Result<String, String>>) -> Self {
        Self {
            chat_id,
            events,
            cancelled: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    fn start_with_program(
        program: PathBuf,
        chat_id: String,
        prompt: &str,
        reply: &str,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (sender, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let request = format!(
            "Conversation to title:\n\nUser: {}\n\nAssistant: {}",
            excerpt(prompt),
            excerpt(reply)
        );
        let worker = thread::spawn(move || {
            let result = summarize_title(&program, &request, &cancel);
            let _ = sender.send(result);
            wake();
        });
        Self {
            chat_id,
            events,
            cancelled,
            worker: Some(worker),
        }
    }
}

impl Drop for TitleTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("MacAi: the title worker unexpectedly panicked.");
            }
        }
    }
}

fn excerpt(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= TITLE_CONTEXT_CHARS {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(TITLE_CONTEXT_CHARS).collect();
    short.push_str("...");
    short
}

fn summarize_title(
    program: &Path,
    request: &str,
    cancelled: &AtomicBool,
) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args([
            "respond",
            "--model",
            ModelChoice::OnDevice.fm_name(),
            "--no-stream",
        ])
        .args(["--instructions", TITLE_INSTRUCTIONS])
        .arg(request)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb");
    let output = process::run(command, cancelled, TITLE_TIMEOUT)?;
    if !output.status.success() {
        return Err(output.diagnostics());
    }
    clean_title(&output.stdout).ok_or_else(|| "The model returned an empty title.".into())
}

/// Turns model output into a one-line plain-text title, or None if nothing usable remains.
pub fn clean_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .unwrap_or(line);
    let plain: String = line
        .chars()
        .filter(|c| !matches!(c, '"' | '“' | '”' | '«' | '»' | '`' | '*' | '_' | '#' | '>'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let plain = plain
        .trim_matches(|c: char| matches!(c, '\'' | '‘' | '’' | '-' | '–' | '—' | ':'))
        .trim_end_matches(['.', '!', '?', ',', ';'])
        .trim();
    if plain.is_empty() {
        return None;
    }
    if plain.chars().count() <= TITLE_MAX_CHARS {
        return Some(plain.to_owned());
    }
    let mut title = String::new();
    for word in plain.split(' ') {
        let next = title.chars().count() + usize::from(!title.is_empty()) + word.chars().count();
        if next > TITLE_MAX_CHARS - 3 {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    if title.is_empty() {
        title = plain.chars().take(TITLE_MAX_CHARS - 3).collect();
    }
    title.push_str("...");
    Some(title)
}

fn read_diagnostics(mut reader: impl Read) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count.min(65536 - bytes.len())]);
    }
    Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
}

#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.pending.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.pending) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err("The model returned invalid UTF-8 text.".into()),
        };
        let text = std::str::from_utf8(&self.pending[..valid])
            .map_err(|error| format!("Cannot decode model output: {error}"))?
            .to_owned();
        self.pending.drain(..valid);
        Ok(text)
    }

    fn finish(&self) -> Result<(), String> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err("The model output ended inside a Unicode character.".into())
        }
    }
}

fn read_stream(
    mut reader: impl Read,
    sender: &Sender<Event>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> Result<String, String> {
    let mut decoder = Utf8Stream::default();
    let mut full = String::new();
    let mut buffer = [0; 2048];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("Cannot read model output: {error}"))?;
        if count == 0 {
            break;
        }
        let text = decoder.push(&buffer[..count])?;
        if !text.is_empty() {
            full.push_str(&text);
            let _ = sender.send(Event::Chunk(text));
            wake();
        }
    }
    decoder.finish()?;
    Ok(full)
}

pub fn is_license_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("license") || lower.contains("legal notice") || lower.contains("not agreed")
}

pub fn is_cloud_context_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("private cloud compute") && lower.contains("not available")
}

fn explain_failure(model: ModelChoice, diagnostics: &str, status: &str) -> String {
    let lower = diagnostics.to_lowercase();
    let hint = if is_cloud_context_error(diagnostics) {
        "Apple Cloud isn't available to MacAi right now. Apple currently only allows Private Cloud Compute from the Terminal app. Switch to On-device to keep chatting."
    } else if lower.contains("context")
        && (lower.contains("exceed") || lower.contains("limit") || lower.contains("window"))
    {
        "This conversation has reached the model's context limit. Start a new chat or use a shorter message."
    } else if lower.contains("guardrail") || lower.contains("refusal") {
        "Apple's model could not answer this request under its safety rules. Try rephrasing it."
    } else if lower.contains("not available")
        || lower.contains("unavailable")
        || lower.contains("not enabled")
    {
        match model {
            ModelChoice::OnDevice => "The on-device model is not ready. Check Apple Intelligence in System Settings, then retry.",
            ModelChoice::Cloud => "Apple Cloud is not available right now. Switch to On-device, or retry later.",
        }
    } else if is_license_error(diagnostics) {
        "Apple's Foundation Models terms need attention. Choose Check again in MacAi to review and accept them."
    } else {
        "The model could not finish this reply. You can retry without losing your previous conversation."
    };
    if diagnostics.is_empty() {
        format!("{hint}\n\nfm exited with {status}.")
    } else {
        format!("{hint}\n\n{diagnostics}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn unicode_survives_every_possible_byte_boundary() {
        let input = "Hello 🦦 — مرحبا café\n";
        for split in 0..=input.len() {
            let mut decoder = Utf8Stream::default();
            let mut output = decoder.push(&input.as_bytes()[..split]).unwrap();
            output.push_str(&decoder.push(&input.as_bytes()[split..]).unwrap());
            decoder.finish().unwrap();
            assert_eq!(output, input);
        }
        assert!(Utf8Stream::default().push(&[0xff]).is_err());
    }

    fn fixture(script: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("fm");
        fs::write(&program, format!("#!/bin/sh\n{script}")).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        (directory, program)
    }

    fn conversation(turns: usize) -> Value {
        let mut entries = vec![serde_json::json!({"role": "instructions", "id": "i"})];
        for turn in 0..turns {
            entries.push(serde_json::json!({"role": "user", "id": format!("u{turn}")}));
            entries.push(serde_json::json!({"role": "response", "id": format!("r{turn}")}));
        }
        serde_json::json!({"modelName": "system", "transcript": {"transcript": {"entries": entries}}})
    }

    fn ids(transcript: &Value) -> Vec<String> {
        entries(transcript)
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn only_recent_turns_and_instructions_are_sent_to_the_model() {
        let full = conversation(9);
        let sent = recent_context(&full, 6);
        assert_eq!(
            ids(&sent),
            ["i", "u3", "r3", "u4", "r4", "u5", "r5", "u6", "r6", "u7", "r7", "u8", "r8"]
        );
        assert_eq!(ids(&full).len(), 19, "the saved copy is not changed");
        assert_eq!(recent_context(&conversation(6), 6), conversation(6));
        let unknown = serde_json::json!({"modelName": "system", "transcript": {"v": 2}});
        assert_eq!(recent_context(&unknown, 6), unknown);
    }

    #[test]
    fn new_turns_are_added_back_onto_the_full_saved_history() {
        let full = conversation(9);
        let sent = recent_context(&full, 6);
        let mut returned = sent.clone();
        let list = returned["transcript"]["transcript"]["entries"]
            .as_array_mut()
            .unwrap();
        list.push(serde_json::json!({"role": "user", "id": "u9"}));
        list.push(serde_json::json!({"role": "response", "id": "r9"}));
        let merged = merge_context(&full, &sent, returned.clone());
        assert_eq!(ids(&merged).len(), 21);
        assert_eq!(&ids(&merged)[17..], ["u8", "r8", "u9", "r9"]);
        assert_eq!(ids(&merged)[..3], ["i", "u0", "r0"]);

        // If fm rewrote the context we sent, trust what it returned.
        let rewritten = conversation(1);
        assert_eq!(merge_context(&full, &sent, rewritten.clone()), rewritten);
    }

    #[test]
    fn long_chats_resume_with_a_trimmed_copy_and_keep_full_history() {
        let (directory, program) = fixture(
            r#"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--save-transcript" ]; then shift; output="$1"; fi
  if [ "$1" = "--resume" ]; then shift; resume="$1"; fi
  shift
done
/bin/cat > /dev/null
/bin/cp "$resume" "$0.sent"
/usr/bin/sed 's/]}}}$/,{"id":"new-u","role":"user"},{"id":"new-r","role":"response"}]}}}/' "$resume" > "$output"
printf 'Fresh reply\n'
"#,
        );
        let full = conversation(10);
        let generation = Generation::start_with_program(
            program.clone(),
            ModelChoice::OnDevice,
            "Next".into(),
            Some(full.clone()),
            directory.path().to_owned(),
            Arc::new(|| {}),
        );
        let (text, saved) = finish(generation).unwrap();
        assert_eq!(text, "Fresh reply");
        let sent: Value =
            serde_json::from_slice(&fs::read(program.with_extension("sent")).unwrap()).unwrap();
        assert_eq!(ids(&sent).len(), 1 + 2 * CONTEXT_TURNS);
        assert_eq!(ids(&sent)[1], "u4");
        assert_eq!(ids(&saved).len(), ids(&full).len() + 2);
        assert_eq!(ids(&saved).last().unwrap(), "new-r");
    }

    #[test]
    fn process_streams_and_returns_a_valid_transcript() {
        let (directory, program) = fixture(
            r#"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--save-transcript" ]; then shift; output="$1"; fi
  shift
done
prompt=$(/bin/cat)
printf '%s\n' "$prompt"
printf '{"modelName":"system","transcript":{"ok":true}}' > "$output"
"#,
        );
        let generation = Generation::start_with_program(
            program,
            ModelChoice::OnDevice,
            "Hello 🦦".into(),
            None,
            directory.path().to_owned(),
            Arc::new(|| {}),
        );
        let mut chunks = String::new();
        loop {
            match generation
                .events
                .recv_timeout(Duration::from_secs(15))
                .unwrap()
            {
                Event::Chunk(text) => chunks.push_str(&text),
                Event::Finished { text, transcript } => {
                    assert_eq!(text, "Hello 🦦");
                    assert_eq!(transcript["modelName"], "system");
                    break;
                }
                event => panic!("Unexpected event: {event:?}"),
            }
        }
        assert_eq!(chunks, "Hello 🦦\n");
    }

    fn finish(generation: Generation) -> Result<(String, Value), String> {
        loop {
            match generation
                .events
                .recv_timeout(Duration::from_secs(15))
                .unwrap()
            {
                Event::Chunk(_) => {}
                Event::Finished { text, transcript } => return Ok((text, transcript)),
                Event::Failed(error) => return Err(error),
                Event::Cancelled => panic!("unexpected cancellation"),
            }
        }
    }

    #[test]
    fn selected_model_is_passed_without_extra_instructions() {
        let (directory, program) = fixture(
            r#"
printf '%s\n' "$*" > "$0.args"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--save-transcript" ]; then shift; output="$1"; fi
  shift
done
/bin/cat > /dev/null
printf 'Hi'
printf '{"modelName":"pcc","transcript":{}}' > "$output"
"#,
        );
        for (model, expected) in [
            (ModelChoice::OnDevice, "system"),
            (ModelChoice::Cloud, "pcc"),
        ] {
            let result = finish(Generation::start_with_program(
                program.clone(),
                model,
                "Hello".into(),
                None,
                directory.path().to_owned(),
                Arc::new(|| {}),
            ));
            let args = fs::read_to_string(program.with_extension("args")).unwrap();
            assert!(
                args.starts_with(&format!("respond --model {expected} --stream")),
                "{args}"
            );
            assert!(!args.contains("--instructions"), "{args}");
            // The on-device model must report itself as `system`.
            assert_eq!(result.is_ok(), model == ModelChoice::Cloud, "{result:?}");
        }
    }

    #[test]
    fn unavailable_cloud_context_is_explained_kindly() {
        let (directory, program) = fixture(
            "echo 'Error: Private Cloud Compute is not available in this context. Please use the Terminal app.' >&2\nexit 1\n",
        );
        let error = finish(Generation::start_with_program(
            program,
            ModelChoice::Cloud,
            "Hello".into(),
            None,
            directory.path().to_owned(),
            Arc::new(|| {}),
        ))
        .unwrap_err();
        assert!(error.contains("Switch to On-device"), "{error}");
        assert!(error.contains("Terminal app"), "{error}");
    }

    #[test]
    fn titles_are_summarized_on_device_without_touching_the_transcript() {
        let (directory, program) = fixture(
            r#"
for arg in "$@"; do printf '%s\n' "$arg"; done > "$0.args"
printf '\n**"Planning a Weekend Trip to Rome."**\nExtra line\n'
"#,
        );
        let task = TitleTask::start_with_program(
            program.clone(),
            "chat".into(),
            "Help me plan 3 days in Rome",
            "Day 1: the Colosseum...",
            || {},
        );
        let title = task
            .events
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
            .unwrap();
        assert_eq!(title, "Planning a Weekend Trip to Rome");
        let args = fs::read_to_string(program.with_extension("args")).unwrap();
        let args: Vec<_> = args.lines().collect();
        assert_eq!(&args[..4], ["respond", "--model", "system", "--no-stream"]);
        assert!(!args.contains(&"--resume") && !args.contains(&"--save-transcript"));
        assert!(args
            .iter()
            .any(|arg| arg.contains("Help me plan 3 days in Rome")));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_or_empty_titles_are_reported_as_errors() {
        for script in ["echo 'guardrail' >&2\nexit 1\n", "printf '  \\n**\\n'\n"] {
            let (_directory, program) = fixture(script);
            let task = TitleTask::start_with_program(program, "chat".into(), "Hi", "Hello", || {});
            assert!(task
                .events
                .recv_timeout(Duration::from_secs(15))
                .unwrap()
                .is_err());
        }
    }

    #[test]
    fn titles_are_cleaned_to_one_short_plain_line() {
        assert_eq!(
            clean_title("Title: \"Rust Error Handling\".").as_deref(),
            Some("Rust Error Handling")
        );
        assert_eq!(
            clean_title("# Café ☕ Ideas!\n\nmore").as_deref(),
            Some("Café ☕ Ideas")
        );
        assert_eq!(clean_title("'Quoted'").as_deref(), Some("Quoted"));
        assert_eq!(clean_title(" \n ** \n"), None);
        let long = clean_title(&"word ".repeat(30)).unwrap();
        assert!(
            long.chars().count() <= TITLE_MAX_CHARS && long.ends_with("..."),
            "{long}"
        );
        let unbroken = clean_title(&"🦦".repeat(80)).unwrap();
        assert!(unbroken.chars().count() <= TITLE_MAX_CHARS, "{unbroken}");
    }

    #[test]
    fn errors_include_model_diagnostics() {
        let (directory, program) = fixture("echo 'context window limit exceeded' >&2\nexit 1\n");
        let generation = Generation::start_with_program(
            program,
            ModelChoice::OnDevice,
            "Hello".into(),
            None,
            directory.path().to_owned(),
            Arc::new(|| {}),
        );
        match generation
            .events
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
        {
            Event::Failed(error) => {
                assert!(error.contains("Start a new chat"));
                assert!(error.contains("context window limit exceeded"));
            }
            event => panic!("Unexpected event: {event:?}"),
        }
    }

    #[test]
    fn cancellation_stops_the_owned_process_and_cleans_temporary_files() {
        let (directory, program) = fixture("exec /bin/sleep 30");
        let generation = Generation::start_with_program(
            program,
            ModelChoice::OnDevice,
            "Hello".into(),
            None,
            directory.path().to_owned(),
            Arc::new(|| {}),
        );
        thread::sleep(Duration::from_millis(100));
        generation.cancel();
        assert!(matches!(
            generation
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            Event::Cancelled
        ));
        drop(generation);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    #[ignore = "requires the real Apple Foundation Model on macOS 27"]
    fn live_model_remembers_context_after_history_is_reopened() {
        use crate::storage::Store;

        fn answer(prompt: &str, transcript: Option<Value>, directory: &Path) -> (String, Value) {
            let generation = Generation::start_with_program(
                FM.into(),
                ModelChoice::OnDevice,
                prompt.into(),
                transcript,
                directory.to_owned(),
                Arc::new(|| {}),
            );
            let mut chunks = 0;
            loop {
                match generation
                    .events
                    .recv_timeout(Duration::from_secs(120))
                    .unwrap()
                {
                    Event::Chunk(_) => chunks += 1,
                    Event::Finished { text, transcript } => {
                        assert!(chunks > 0, "a real reply must be streamed");
                        return (text, transcript);
                    }
                    event => panic!("Local inference failed: {event:?}"),
                }
            }
        }

        assert!(matches!(
            crate::setup::check(&AtomicBool::new(false)).unwrap(),
            crate::setup::Readiness::Ready(_)
        ));
        let directory = tempfile::tempdir().unwrap();
        let (store, mut history) = Store::open(directory.path().to_owned()).unwrap();
        let prompt = "Remember this phrase for our conversation: violet otter. Reply with only OK.";
        history.selected_mut().begin(prompt.into());
        let (text, transcript) = answer(prompt, None, directory.path());
        history.selected_mut().finish(text, transcript);
        store.save(&history).unwrap();
        drop(store);

        let (store, mut reopened) = Store::open(directory.path().to_owned()).unwrap();
        let prompt = "What phrase did I ask you to remember? Reply with only that phrase.";
        let context = reopened.selected().transcript.clone();
        reopened.selected_mut().begin(prompt.into());
        let (text, transcript) = answer(prompt, context, directory.path());
        let normalized = text.to_lowercase();
        assert!(
            normalized.contains("violet") && normalized.contains("otter"),
            "{text}"
        );
        reopened.selected_mut().finish(text.clone(), transcript);
        store.save(&reopened).unwrap();
        println!("Real model resumed saved history and replied: {text}");
        assert_eq!(reopened.selected().messages.len(), 4);
    }
}
