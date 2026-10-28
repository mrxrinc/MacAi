use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

const IDENTITY: &str = "Developer ID Application: MacAi Test (TESTTEAM)";

fn executable(path: &Path, contents: &str) {
    fs::write(path, format!("#!/bin/sh\nset -eu\n{contents}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("scripts")).unwrap();
    fs::create_dir(root.path().join("bin")).unwrap();
    fs::write(
        root.path().join("scripts/release.sh"),
        include_str!("../scripts/release.sh"),
    )
    .unwrap();
    executable(
        &root.path().join("scripts/build.sh"),
        "printf 'build\\n' >> \"$RELEASE_LOG\"\nmkdir -p dist/MacAi.app\n",
    );
    executable(
        &root.path().join("bin/security"),
        &format!("printf '  1) ABCDEF \"{IDENTITY}\"%s\\n' \"${{CERT_ERROR:-}}\""),
    );
    executable(
        &root.path().join("bin/codesign"),
        "printf 'codesign %s\\n' \"$*\" >> \"$RELEASE_LOG\"",
    );
    executable(
        &root.path().join("bin/xcrun"),
        r#"
printf 'xcrun %s\n' "$*" >> "$RELEASE_LOG"
case "$1/$2" in
    notarytool/submit) printf '{"id":"test-id","status":"%s"}\n' "${NOTARY_STATUS:-Accepted}" ;;
    notarytool/log) printf '{}\n' > "$6" ;;
    stapler/staple) touch "$3/stapled" ;;
    stapler/validate) test -f "$3/stapled" ;;
    *) exit 2 ;;
esac
"#,
    );
    executable(
        &root.path().join("bin/spctl"),
        r#"
printf 'spctl %s\n' "$*" >> "$RELEASE_LOG"
if [ "${ASSESSMENT_FAIL:-0}" = 1 ]; then exit 1; fi
"#,
    );
    executable(
        &root.path().join("bin/ditto"),
        r#"
printf 'ditto %s\n' "$*" >> "$RELEASE_LOG"
if [ -f "$5/stapled" ]; then printf 'stapled app\n' > "$6"; else printf 'unsigned ticket\n' > "$6"; fi
"#,
    );
    root
}

fn release(root: &Path) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("scripts/release.sh")
        .current_dir(root)
        .env(
            "PATH",
            format!(
                "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                root.join("bin").display()
            ),
        )
        .env("MACAI_SIGNING_IDENTITY", IDENTITY)
        .env("MACAI_NOTARY_PROFILE", "test-profile")
        .env("RELEASE_LOG", root.join("calls.log"));
    command
}

#[test]
fn release_requires_the_right_certificate_before_building() {
    for identity in ["", "Apple Development: Test", IDENTITY] {
        let directory = fixture();
        let output = release(directory.path())
            .env("MACAI_SIGNING_IDENTITY", identity)
            .env("CERT_ERROR", " (CSSMERR_TP_CERT_REVOKED)")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!directory.path().join("calls.log").exists());
    }
}

#[test]
fn only_a_notarized_stapled_and_assessed_app_is_published() {
    let directory = fixture();
    let output = release(directory.path()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("dist/MacAi.zip")).unwrap(),
        "stapled app\n"
    );
    let log = fs::read_to_string(directory.path().join("calls.log")).unwrap();
    assert!(log.contains("--options runtime --timestamp"));
    let submit = log.find("notarytool submit").unwrap();
    let staple = log.find("stapler staple").unwrap();
    let assess = log.find("spctl --assess").unwrap();
    let archive = log.rfind("ditto").unwrap();
    assert!(submit < staple && staple < assess && assess < archive);
}

#[test]
fn rejection_or_assessment_failure_never_publishes_an_archive() {
    for (variable, value) in [("NOTARY_STATUS", "Invalid"), ("ASSESSMENT_FAIL", "1")] {
        let directory = fixture();
        let output = release(directory.path())
            .env(variable, value)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!directory.path().join("dist/MacAi.zip").exists());
    }
}
