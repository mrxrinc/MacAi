use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub struct ManagedChild(pub Child);

impl Drop for ManagedChild {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
}

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn diagnostics(&self) -> String {
        format!("{}\n{}", self.stdout.trim(), self.stderr.trim())
            .trim()
            .to_owned()
    }
}

pub fn run(
    mut command: Command,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<Output, String> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("Setup was cancelled.".into());
    }
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = ManagedChild(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start {program}: {error}"))?,
    );
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or("Missing command output pipe.")?;
    let stderr = child
        .0
        .stderr
        .take()
        .ok_or("Missing command diagnostics pipe.")?;
    let out = thread::spawn(move || read_bounded(stdout));
    let err = thread::spawn(move || read_bounded(stderr));
    let start = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::SeqCst) {
            break Err("Setup was cancelled.".to_owned());
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(format!("Cannot check {program}: {error}")),
        }
        if start.elapsed() >= timeout {
            break Err(format!(
                "{program} did not finish within {} seconds. Please retry.",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };
    // Close the process before joining readers, including on timeout or cancellation.
    drop(child);
    let stdout = out
        .join()
        .map_err(|_| "The command output reader stopped unexpectedly.")?;
    let stderr = err
        .join()
        .map_err(|_| "The command diagnostics reader stopped unexpectedly.")?;
    Ok(Output {
        status: status?,
        stdout: stdout?,
        stderr: stderr?,
    })
}

fn read_bounded(mut reader: impl Read) -> Result<String, String> {
    const LIMIT: usize = 1024 * 1024;
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let mut too_large = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        let remaining = LIMIT - bytes.len();
        too_large |= count > remaining;
        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    if too_large {
        return Err("The command returned too much text to display safely.".into());
    }
    String::from_utf8(bytes)
        .map_err(|error| format!("The command returned unreadable text: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_bounded_and_cancelled_without_waiting_for_completion() {
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        assert!(run(command, &cancel, Duration::from_millis(100))
            .unwrap_err()
            .contains("did not finish"));
        assert!(start.elapsed() < Duration::from_secs(3));
        cancel.store(true, Ordering::SeqCst);
        assert!(
            run(Command::new("/bin/echo"), &cancel, Duration::from_secs(1))
                .unwrap_err()
                .contains("cancelled")
        );
    }

    #[test]
    fn oversized_or_invalid_output_is_not_silently_truncated() {
        assert!(read_bounded(&[0xff][..]).is_err());
        assert!(read_bounded(&vec![b'a'; 1024 * 1024 + 1][..]).is_err());
    }
}
