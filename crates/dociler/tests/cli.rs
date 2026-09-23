use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

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
            .env("DOCILER_CONFIG_DIR", self.0.join("settings"))
            .env("DOCILER_DATA_DIR", self.0.join("data"))
            .env("DOCILER_CACHE_DIR", self.0.join("cache"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run dociler")
    }

    fn run_with_input(&self, args: &[&str], input: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dociler"))
            .current_dir(&self.0)
            .env("DOCILER_CONFIG_DIR", self.0.join("settings"))
            .env("DOCILER_DATA_DIR", self.0.join("data"))
            .env("DOCILER_CACHE_DIR", self.0.join("cache"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run dociler");
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().expect("wait for dociler")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove isolated test workspace");
    }
}

fn read_request(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0);
        request.extend_from_slice(&buffer[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .map(str::to_owned)
        })
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while request.len() - header_end < length {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0);
        request.extend_from_slice(&buffer[..count]);
    }
    assert!(!String::from_utf8_lossy(&request).contains("private-test-value"));
}

fn serve_cli_flow() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let replies = [
            ("application/json", r#"{"data":[{"id":"model-a"}]}"#),
            (
                "application/json",
                r#"{"choices":[{"message":{"content":"OK"}}]}"#,
            ),
            (
                "text/event-stream",
                "data: {\"choices\":[{\"delta\":{\"content\":\"CLI works\"}}]}\n\ndata: [DONE]\n\n",
            ),
            ("application/json", r#"{"data":[{"id":"model-a"}]}"#),
            (
                "application/json",
                r#"{"choices":[{"message":{"content":"OK"}}]}"#,
            ),
            ("application/json", r#"{"data":[{"id":"model-a"}]}"#),
            (
                "application/json",
                r#"{"choices":[{"message":{"content":"OK"}}]}"#,
            ),
            ("application/json", r#"{"data":[{"id":"model-b"}]}"#),
            (
                "application/json",
                r#"{"choices":[{"message":{"content":"OK"}}]}"#,
            ),
        ];
        for (content_type, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (format!("http://{address}/v1"), handle)
}

#[test]
fn non_interactive_launch_prints_help_without_creating_state() {
    let workspace = Workspace::new();
    let output = workspace.run(&[]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Usage: dociler"));
    assert!(text.contains("remote text chat are available"));
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
    assert!(text.contains("Hardware inventory: read-only; not persisted"));
    assert!(text.contains("dociler-lite:"));
    assert!(text.contains("dociler-pro:"));
    assert!(text.contains("Preflight is not final admission"));
    assert!(!text.contains("rahasia.txt"));
    assert!(!text.contains("confidential fixture"));
    assert_eq!(fs::read(&path).unwrap(), b"confidential fixture");
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 1);
}

#[test]
fn model_status_is_read_only_and_reports_both_preflight_tiers() {
    let workspace = Workspace::new();
    let output = workspace.run(&["model", "status"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Memory:"));
    assert!(text.contains("CPU:"));
    assert!(text.contains("Asset storage:"));
    assert!(text.contains("dociler-lite:"));
    assert!(text.contains("dociler-pro:"));
    assert!(text.contains("no runtime/model was downloaded or executed"));
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn model_manifest_listing_and_verification_are_read_only() {
    let workspace = Workspace::new();
    let list = workspace.run(&["model", "list"]);
    assert!(list.status.success());
    assert!(list.stderr.is_empty());
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.contains("dociler-assets-v1"));
    assert!(text.contains("dociler-lite:"));
    assert!(text.contains("dociler-pro:"));
    assert!(text.contains("llama.cpp runtime: v0.4.0/b10809"));
    assert!(text.contains("cache=missing"));
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);

    let verify = workspace.run(&["model", "verify", "dociler-lite"]);
    assert_eq!(verify.status.code(), Some(1));
    let verified = String::from_utf8(verify.stdout).unwrap();
    assert!(verified.contains("dociler-lite:"));
    assert!(!verified.contains("dociler-pro:"));
    assert!(verified.contains("Verification was read-only"));
    assert!(
        String::from_utf8(verify.stderr)
            .unwrap()
            .contains("missing or invalid")
    );
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn model_download_requires_consent_before_creating_state() {
    let workspace = Workspace::new();
    let output = workspace.run(&["model", "download", "dociler-lite"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("explicit consent"));
    assert!(error.contains("--confirm"));
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn runtime_install_requires_consent_before_creating_state() {
    let workspace = Workspace::new();
    let output = workspace.run(&["model", "runtime-install"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("explicit consent"));
    assert!(error.contains("--confirm"));
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn runtime_probe_requires_consent_and_an_installed_runtime() {
    let workspace = Workspace::new();
    let denied = workspace.run(&["model", "runtime-probe"]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(denied.stdout.is_empty());
    let error = String::from_utf8(denied.stderr).unwrap();
    assert!(error.contains("explicit consent"));
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);

    let missing = workspace.run(&["model", "runtime-probe", "--confirm"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("not installed")
    );
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn model_load_probe_requires_consent_and_verified_assets() {
    let workspace = Workspace::new();
    let denied = workspace.run(&["model", "load-probe", "dociler-lite"]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(denied.stdout.is_empty());
    assert!(
        String::from_utf8(denied.stderr)
            .unwrap()
            .contains("explicit consent")
    );
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);

    let missing = workspace.run(&["model", "load-probe", "dociler-lite", "--confirm"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("GGUF is missing")
    );
    assert!(!workspace.0.join("data").exists());
}

#[test]
fn diagnostics_reject_a_file_as_workspace() {
    let workspace = Workspace::new();
    let file = workspace.0.join("input.txt");
    fs::write(&file, "data").unwrap();
    assert!(dociler_core::diagnostics::Diagnostics::inspect(&file).is_err());
    assert!(dociler_core::diagnostics::Diagnostics::inspect(&workspace.0.join("missing")).is_err());
}

#[test]
fn config_inspection_creates_nothing_and_init_is_explicit() {
    let workspace = Workspace::new();
    for args in [["config", "paths"], ["config", "show"]] {
        let output = workspace.run(&args);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
    }
    let output = workspace.run(&["config", "init"]);
    assert!(output.status.success());
    let file = workspace.0.join("settings/config.json");
    let before = fs::read(&file).unwrap();
    let output = workspace.run(&["config", "init"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Nothing overwritten")
    );
    assert_eq!(fs::read(&file).unwrap(), before);
    let output = workspace.run(&["doctor"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("schema v1 (validated)"));
    assert!(text.contains("read-only"));
    assert!(text.contains("memory only"));
}

#[test]
fn invalid_saved_config_blocks_inspection_without_disclosing_its_contents() {
    let workspace = Workspace::new();
    assert!(workspace.run(&["config", "init"]).status.success());
    let file = workspace.0.join("settings/config.json");
    let bytes = br#"{"schema_version":1,"api_key":"private-test-value"}"#;
    fs::write(&file, bytes).unwrap();
    for args in [vec!["doctor"], vec!["config", "show"]] {
        let output = workspace.run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("No defaults substituted"));
        assert!(!error.contains("private-test-value"));
    }
    // Help and path recovery commands still work with broken config.
    assert!(workspace.run(&["--help"]).status.success());
    assert!(workspace.run(&["config", "paths"]).status.success());
    assert_eq!(fs::read(&file).unwrap(), bytes);
}

#[test]
fn application_directory_overrides_must_be_absolute() {
    let workspace = Workspace::new();
    for variable in [
        "DOCILER_CONFIG_DIR",
        "DOCILER_DATA_DIR",
        "DOCILER_CACHE_DIR",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dociler"))
            .current_dir(&workspace.0)
            .env(variable, "relative-secret-value")
            .args(["config", "paths"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(
            !String::from_utf8(output.stderr)
                .unwrap()
                .contains("relative-secret-value")
        );
    }
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
}

#[test]
fn remote_profile_add_list_and_stdin_run_work_end_to_end() {
    let workspace = Workspace::new();
    let (url, server) = serve_cli_flow();
    let add = workspace.run(&["connect", "add", "office", &url, "model-a"]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    assert!(
        String::from_utf8(add.stdout)
            .unwrap()
            .contains("Saved remote profile 'office'")
    );
    let list = workspace.run(&["connect", "list"]);
    let listed = String::from_utf8(list.stdout).unwrap();
    assert!(listed.contains("office"));
    assert!(listed.contains("credential=none"));
    let run = workspace.run_with_input(&["run", "office"], "Tolong jawab".as_bytes());
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "CLI works\n");
    assert!(run.stderr.is_empty());
    let config_path = workspace.0.join("settings/config.json");
    let config = fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("office"));
    assert!(!config.contains("Tolong jawab"));

    let missing_key = workspace.run(&["connect", "key", "office"]);
    assert_eq!(missing_key.status.code(), Some(1));
    assert!(
        String::from_utf8(missing_key.stderr)
            .unwrap()
            .contains("DOCILER_API_KEY")
    );
    assert!(fs::read_to_string(&config_path).unwrap().contains("office"));
    let keyless = workspace.run(&["connect", "key-clear", "office", "--confirm"]);
    assert!(
        keyless.status.success(),
        "{}",
        String::from_utf8_lossy(&keyless.stderr)
    );
    assert!(
        String::from_utf8(keyless.stdout)
            .unwrap()
            .contains("updated credential policy")
    );
    let checked = workspace.run(&["connect", "check", "office"]);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(String::from_utf8(checked.stdout).unwrap().contains("ready"));
    let edited = workspace.run(&["connect", "edit", "office", &url, "model-b"]);
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    assert!(
        String::from_utf8(edited.stdout)
            .unwrap()
            .contains("updated")
    );
    assert!(
        fs::read_to_string(&config_path)
            .unwrap()
            .contains("model-b")
    );
    server.join().unwrap();
    let unconfirmed = workspace.run(&["connect", "remove", "office"]);
    assert_eq!(unconfirmed.status.code(), Some(1));
    assert!(
        String::from_utf8(unconfirmed.stderr)
            .unwrap()
            .contains("--confirm")
    );
    assert!(fs::read_to_string(&config_path).unwrap().contains("office"));
    let removed = workspace.run(&["connect", "remove", "office", "--confirm"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(
        String::from_utf8(removed.stdout)
            .unwrap()
            .contains("Removed")
    );
    assert!(!fs::read_to_string(&config_path).unwrap().contains("office"));
}

#[test]
fn unsafe_remote_and_missing_profile_fail_without_writes() {
    let workspace = Workspace::new();
    let output = workspace.run(&["connect", "verify", "http://8.8.8.8/v1", "model"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("unsafe endpoint")
    );
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
    let output = workspace.run_with_input(&["run", "missing"], b"prompt");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
    let output = workspace.run(&["chat", "missing"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 0);
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
