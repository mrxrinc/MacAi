use crate::process::ManagedChild;
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

const FM: &str = "/usr/bin/fm";
const INSTRUCTIONS: &str = "You are a helpful assistant running privately on the user's Mac. \
     Give clear, thoughtful answers. Use Markdown when it improves readability. \
     You cannot access the internet, the user's files, or run commands.";
const GENERATION_TIMEOUT: Duration = Duration::from_secs(300);

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
    pub fn start(
        prompt: String,
        transcript: Option<Value>,
        directory: PathBuf,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::start_with_program(
            PathBuf::from(FM),
            prompt,
            transcript,
            directory,
            Arc::new(wake),
        )
    }

    fn start_with_program(
        program: PathBuf,
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
                &program, &prompt, transcript, &directory, &cancel, &sender, &wake,
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

fn run(
    program: &Path,
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
    let mut command = Command::new(program);
    command
        .args(["respond", "--model", "system", "--stream"])
        .arg("--save-transcript")
        .arg(&output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(transcript) = transcript {
        let input = temporary.path().join("context.json");
        let bytes = serde_json::to_vec(&transcript)
            .map_err(|error| format!("Cannot encode conversation context: {error}"))?;
        fs::write(&input, bytes)
            .map_err(|error| format!("Cannot prepare conversation context: {error}"))?;
        command.arg("--resume").arg(input);
    } else {
        command.args(["--instructions", INSTRUCTIONS]);
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
        return Err(explain_failure(&diagnostics, &status.to_string()));
    }
    input_result.map_err(|error| format!("Cannot send your message to the model: {error}"))?;
    let text = text?.trim_end().to_owned();
    if text.is_empty() {
        return Err(
            "The local model returned an empty reply. Try again or start a new conversation."
                .into(),
        );
    }
    let bytes = fs::read(&output).map_err(|error| {
        format!("The reply arrived, but its conversation context could not be saved: {error}")
    })?;
    let transcript: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!("The model returned an unreadable conversation transcript: {error}")
    })?;
    if transcript.get("transcript").is_none()
        || transcript.get("modelName").and_then(Value::as_str) != Some("system")
    {
        return Err("The model returned an unexpected transcript format. Your previous conversation context is unchanged.".into());
    }
    Ok(Some((text, transcript)))
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

fn explain_failure(diagnostics: &str, status: &str) -> String {
    let lower = diagnostics.to_lowercase();
    let hint = if lower.contains("context")
        && (lower.contains("exceed") || lower.contains("limit") || lower.contains("window"))
    {
        "This conversation has reached the local model's context limit. Start a new chat or use a shorter message."
    } else if lower.contains("guardrail") || lower.contains("refusal") {
        "Apple's on-device model could not answer this request under its safety rules. Try rephrasing it."
    } else if lower.contains("not available")
        || lower.contains("unavailable")
        || lower.contains("not enabled")
    {
        "The local model is not ready. Check Apple Intelligence in System Settings, then retry."
    } else if is_license_error(diagnostics) {
        "Apple's Foundation Models terms need attention. Choose Check again in MacAi to review and accept them."
    } else {
        "The local model could not finish this reply. You can retry without losing your previous conversation."
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

    #[test]
    fn errors_include_model_diagnostics() {
        let (directory, program) = fixture("echo 'context window limit exceeded' >&2\nexit 1\n");
        let generation = Generation::start_with_program(
            program,
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
            let generation =
                Generation::start(prompt.into(), transcript, directory.to_owned(), || {});
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
            crate::setup::Readiness::Ready
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
