use crate::process::{self, Output};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const FM: &str = "/usr/bin/fm";
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(180);
const AUTHORIZATION_SCRIPT: &str =
    "on run argv\n    do shell script (item 1 of argv) with administrator privileges\nend run";
const ACCEPT_SCRIPT: &str = include_str!("../assets/accept-license.exp");

#[derive(Clone, Debug)]
pub struct LicenseDocument {
    pub text: String,
    digest: String,
}

impl LicenseDocument {
    pub(crate) fn new(text: String) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Err(
                "Apple's license text is empty. MacAi cannot request consent to an empty document."
                    .into(),
            );
        }
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        Ok(Self { text, digest })
    }
}

#[derive(Debug)]
pub enum Readiness {
    Ready,
    LicenseRequired(LicenseDocument),
    ModelUnavailable(String),
}

pub struct SetupTask {
    pub events: Receiver<Result<Readiness, String>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SetupTask {
    pub fn check(wake: impl Fn() + Send + 'static) -> Self {
        Self::start(None, wake)
    }

    // Only the GUI's explicit, confirmed Accept action may call this entry point.
    pub fn accept(document: LicenseDocument, wake: impl Fn() + Send + 'static) -> Self {
        Self::start(Some(document), wake)
    }

    fn start(document: Option<LicenseDocument>, wake: impl Fn() + Send + 'static) -> Self {
        let (sender, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let worker = thread::spawn(move || {
            let result = if let Some(document) = document {
                accept_with(Path::new(FM), document, &cancel, authorize)
            } else {
                check(&cancel)
            };
            let _ = sender.send(result);
            wake();
        });
        Self {
            events,
            cancelled,
            worker: Some(worker),
        }
    }
}

impl Drop for SetupTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("MacAi: the setup worker unexpectedly panicked.");
            }
        }
    }
}

fn fm_command(program: &Path, args: &[&str], cancelled: &AtomicBool) -> Result<Output, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("LC_ALL", "C");
    process::run(command, cancelled, CHECK_TIMEOUT).map_err(|error| {
        format!("{error}\nMacAi requires macOS 27 with Apple's /usr/bin/fm command.")
    })
}

fn license_agreed(program: &Path, cancelled: &AtomicBool) -> Result<bool, String> {
    let output = fm_command(program, &["license", "--status"], cancelled)?;
    let status = output.stdout.trim();
    if output.status.success() && status.starts_with("Agreed to license ") {
        Ok(true)
    } else if status == "Not agreed. Run 'sudo fm license' to review and agree." {
        Ok(false)
    } else {
        Err(format!(
            "Could not determine whether Apple's terms have been accepted. MacAi cannot enable chat until this is verified.\n\n{}",
            output.diagnostics()
        ))
    }
}

fn license_document(program: &Path, cancelled: &AtomicBool) -> Result<LicenseDocument, String> {
    let output = fm_command(program, &["license", "--show"], cancelled)?;
    if !output.status.success() {
        return Err(format!(
            "Could not load Apple's license text.\n\n{}",
            output.diagnostics()
        ));
    }
    LicenseDocument::new(output.stdout)
}

pub fn check(cancelled: &AtomicBool) -> Result<Readiness, String> {
    check_with(Path::new(FM), cancelled)
}

#[cfg(feature = "screenshot")]
pub fn preview_license() -> Result<Readiness, String> {
    license_document(Path::new(FM), &AtomicBool::new(false)).map(Readiness::LicenseRequired)
}

fn check_with(program: &Path, cancelled: &AtomicBool) -> Result<Readiness, String> {
    if !license_agreed(program, cancelled)? {
        return license_document(program, cancelled).map(Readiness::LicenseRequired);
    }
    Ok(match check_model_with(program, cancelled) {
        Ok(_) => Readiness::Ready,
        Err(error) => Readiness::ModelUnavailable(error),
    })
}

fn check_model_with(program: &Path, cancelled: &AtomicBool) -> Result<String, String> {
    let output = fm_command(program, &["available"], cancelled)?;
    if output.status.success() && output.stdout.trim() == "System model available" {
        Ok(output.stdout.trim().to_owned())
    } else {
        Err(format!(
            "Apple Intelligence is not ready. Enable it in System Settings and let the model finish downloading, then choose Check again.\n\n{}",
            output.diagnostics()
        ))
    }
}

fn accept_with(
    program: &Path,
    document: LicenseDocument,
    cancelled: &AtomicBool,
    authorize: impl FnOnce(&LicenseDocument, &AtomicBool) -> Result<(), String>,
) -> Result<Readiness, String> {
    if license_agreed(program, cancelled)? {
        return check_with(program, cancelled);
    }
    let current = license_document(program, cancelled)?;
    if current.digest != document.digest {
        return Ok(Readiness::LicenseRequired(current));
    }
    authorize(&document, cancelled)?;
    if !license_agreed(program, cancelled)? {
        return Err("Apple's terms are still not accepted. The administrator approval or license command did not complete; you can retry.".into());
    }
    check_with(program, cancelled)
}

fn authorization_command(document: &LicenseDocument) -> Command {
    let script = ACCEPT_SCRIPT.replace("__LICENSE_SHA256__", &document.digest);
    let shell = format!("/usr/bin/expect -c '{}'", script.replace('\'', "'\\''"));
    let mut command = Command::new("/usr/bin/osascript");
    command.args(["-e", AUTHORIZATION_SCRIPT, &shell]);
    command
}

fn authorize(document: &LicenseDocument, cancelled: &AtomicBool) -> Result<(), String> {
    let output = process::run(authorization_command(document), cancelled, APPROVAL_TIMEOUT)?;
    if output.status.success() {
        Ok(())
    } else if output.stderr.contains("(-128)") {
        Err("Administrator approval was cancelled. You can try again when you are ready.".into())
    } else {
        Err(format!(
            "macOS could not complete the license agreement. An administrator must approve it for this Mac.\n\n{}",
            output.diagnostics()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    const PROMPT: &str = "DO YOU AGREE TO THESE TERMS? ANY RESPONSE OTHER THAN 'yes' OR 'y' WILL DISAGREE AND EXIT. [yes, no]";

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("fm");
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$0.calls"
case "$1/$2" in
    license/--status)
        if [ -f "$0.accepted" ]; then
            printf 'Agreed to license FM1 version 1.0 on test date.\n'
        else
            printf "Not agreed. Run 'sudo fm license' to review and agree.\n"
            exit 1
        fi ;;
    license/--show) /bin/cat "$0.terms" ;;
    license/)
        test -t 0 || exit 2
        printf '%s' "{PROMPT}"
        IFS= read -r answer
        test "$answer" = yes || exit 3
        /usr/bin/touch "$0.accepted" ;;
    available/)
        if [ -f "$0.unavailable" ]; then
            printf 'System model unavailable\n' >&2
            exit 1
        fi
        printf 'System model available\n' ;;
    *) exit 4 ;;
esac
"#
        );
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(program.with_extension("terms"), "Apple test terms\n").unwrap();
        (directory, program)
    }

    fn document() -> LicenseDocument {
        LicenseDocument::new("Apple test terms\n".into()).unwrap()
    }

    fn fake_authorize(
        program: &Path,
        document: &LicenseDocument,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let script = ACCEPT_SCRIPT
            .replace("__LICENSE_SHA256__", &document.digest)
            .replace(
                "set fm /usr/bin/fm",
                &format!("set fm {{{}}}", program.display()),
            );
        let mut command = Command::new("/usr/bin/expect");
        command.args(["-c", &script]);
        let output = process::run(command, cancel, Duration::from_secs(15))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(output.diagnostics())
        }
    }

    #[test]
    fn first_run_loads_terms_without_accepting_or_checking_inference() {
        let (_directory, program) = fixture();
        let result = check_with(&program, &AtomicBool::new(false)).unwrap();
        assert!(
            matches!(result, Readiness::LicenseRequired(doc) if doc.text == "Apple test terms\n")
        );
        assert!(!program.with_extension("accepted").exists());
        let calls = fs::read_to_string(program.with_extension("calls")).unwrap();
        assert_eq!(calls, "license --status\nlicense --show\n");
    }

    #[test]
    fn explicit_acceptance_uses_a_terminal_and_checks_the_recorded_status() {
        let (_directory, program) = fixture();
        let cancel = AtomicBool::new(false);
        let result = accept_with(&program, document(), &cancel, |doc, cancel| {
            fake_authorize(&program, doc, cancel)
        })
        .unwrap();
        assert!(matches!(result, Readiness::Ready));
        assert!(program.with_extension("accepted").exists());
    }

    #[test]
    fn cancelled_approval_and_false_success_do_not_enable_chat() {
        let (_directory, program) = fixture();
        let cancel = AtomicBool::new(false);
        let error = accept_with(&program, document(), &cancel, |_, _| {
            Err("Approval cancelled".into())
        })
        .unwrap_err();
        assert_eq!(error, "Approval cancelled");
        assert!(!program.with_extension("accepted").exists());
        let error = accept_with(&program, document(), &cancel, |_, _| Ok(())).unwrap_err();
        assert!(error.contains("still not accepted"));
    }

    #[test]
    fn changed_terms_require_new_consent_in_both_processes() {
        let (_directory, program) = fixture();
        fs::write(program.with_extension("terms"), "New terms\n").unwrap();
        let cancel = AtomicBool::new(false);
        let result = accept_with(&program, document(), &cancel, |_, _| {
            panic!("must not authorize")
        })
        .unwrap();
        assert!(matches!(result, Readiness::LicenseRequired(doc) if doc.text == "New terms\n"));
        let error = fake_authorize(&program, &document(), &cancel).unwrap_err();
        assert!(error.contains("license changed"));
        assert!(!program.with_extension("accepted").exists());
    }

    #[test]
    fn existing_machine_agreement_needs_no_administrator_prompt() {
        let (_directory, program) = fixture();
        fs::write(program.with_extension("accepted"), "").unwrap();
        let result = accept_with(&program, document(), &AtomicBool::new(false), |_, _| {
            panic!("must not authorize")
        })
        .unwrap();
        assert!(matches!(result, Readiness::Ready));
    }

    #[test]
    fn unknown_status_and_empty_terms_fail_closed() {
        let (_directory, program) = fixture();
        fs::write(program.with_extension("terms"), "").unwrap();
        assert!(check_with(&program, &AtomicBool::new(false))
            .unwrap_err()
            .contains("empty"));
        fs::write(&program, "#!/bin/sh\nprintf 'Unknown status\\n'\n").unwrap();
        assert!(check_with(&program, &AtomicBool::new(false))
            .unwrap_err()
            .contains("Could not determine"));
    }

    #[test]
    fn privileged_command_contains_no_user_text_or_password() {
        let doc =
            LicenseDocument::new("Terms with 'quotes' and $(not-a-command)\n".into()).unwrap();
        let command = authorization_command(&doc);
        assert_eq!(command.get_program(), "/usr/bin/osascript");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(args[1].contains("with administrator privileges"));
        assert!(args[2].contains(&doc.digest));
        assert!(!args[2].contains("not-a-command"));
        assert!(!args[1].contains("password"));
    }

    #[test]
    fn applescript_transports_the_helper_without_opening_terminal() {
        let (_directory, program) = fixture();
        let command = authorization_command(&document());
        let shell = command
            .get_args()
            .nth(2)
            .unwrap()
            .to_string_lossy()
            .replace(
                "set fm /usr/bin/fm",
                &format!("set fm {{{}}}", program.display()),
            );
        let mut unprivileged = Command::new("/usr/bin/osascript");
        unprivileged.args([
            "-e",
            &AUTHORIZATION_SCRIPT.replace(" with administrator privileges", ""),
            &shell,
        ]);
        let output = process::run(
            unprivileged,
            &AtomicBool::new(false),
            Duration::from_secs(15),
        )
        .unwrap();
        assert!(output.status.success(), "{}", output.diagnostics());
        assert!(program.with_extension("accepted").exists());
    }

    #[test]
    fn accepted_terms_do_not_hide_an_unavailable_model() {
        let (_directory, program) = fixture();
        fs::write(program.with_extension("accepted"), "").unwrap();
        fs::write(program.with_extension("unavailable"), "").unwrap();
        let result = check_with(&program, &AtomicBool::new(false)).unwrap();
        assert!(
            matches!(result, Readiness::ModelUnavailable(error) if error.contains("System Settings"))
        );
    }
}
