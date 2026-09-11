use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dociler-test-{}-{}-dokumen ruang",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create isolated test workspace");
        Self(path)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dociler"))
            .current_dir(&self.0)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run dociler")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove isolated test workspace");
    }
}

#[test]
fn non_interactive_launch_prints_help_without_creating_state() {
    let workspace = Workspace::new();
    let output = workspace.run(&[]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Usage: dociler"));
    assert!(text.contains("not available yet"));
    assert!(!text.contains('\u{1b}'));
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn help_and_version_work_outside_the_repository() {
    let workspace = Workspace::new();
    for args in [
        vec!["help"],
        vec!["--help"],
        vec!["-h"],
        vec!["doctor", "--help"],
    ] {
        let output = workspace.run(&args);
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains("Usage:"));
    }
    for flag in ["--version", "-V"] {
        let output = workspace.run(&[flag]);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            concat!("dociler ", env!("CARGO_PKG_VERSION"), "\n")
        );
    }
}

#[test]
fn invalid_arguments_fail_without_echoing_sensitive_input() {
    let workspace = Workspace::new();
    for args in [
        vec!["--api-key=private-test-value"],
        vec!["doctor", "unexpected"],
        vec!["serve"],
    ] {
        let output = workspace.run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("--help"));
        assert!(!error.contains("private-test-value"));
    }
}

#[test]
fn doctor_reports_workspace_without_touching_documents() {
    let workspace = Workspace::new();
    let path = workspace.0.join("rahasia.txt");
    fs::write(&path, "confidential fixture").unwrap();
    let output = workspace.run(&["doctor"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(&format!("{:?}", workspace.0.canonicalize().unwrap())));
    assert!(text.contains("read-only"));
    assert!(text.contains("not assessed"));
    assert!(!text.contains("rahasia.txt"));
    assert!(!text.contains("confidential fixture"));
    assert_eq!(fs::read(&path).unwrap(), b"confidential fixture");
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 1);
}

#[test]
fn diagnostics_reject_a_file_as_workspace() {
    let workspace = Workspace::new();
    let file = workspace.0.join("input.txt");
    fs::write(&file, "data").unwrap();
    assert!(dociler_core::diagnostics::Diagnostics::inspect(&file).is_err());
    assert!(dociler_core::diagnostics::Diagnostics::inspect(&workspace.0.join("missing")).is_err());
}

#[cfg(unix)]
#[test]
fn non_utf8_arguments_are_rejected_without_panicking() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let workspace = Workspace::new();
    let output = Command::new(env!("CARGO_BIN_EXE_dociler"))
        .current_dir(&workspace.0)
        .arg(OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
