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
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input);
        }
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
        vec!["unsupported-subcommand"],
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
    assert!(text.contains("Runtime CPU instructions:"));
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
    assert!(text.contains("required-cpu="));
    assert!(text.contains("expected-libraries="));
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
fn model_load_probe_supports_experimental_and_ceiling_bytes_flags() {
    let workspace = Workspace::new();

    // Unconfirmed with --experimental fails with explicit consent error
    let unconfirmed_exp = workspace.run(&["model", "load-probe", "dociler-lite", "--experimental"]);
    assert_eq!(unconfirmed_exp.status.code(), Some(1));
    assert!(
        String::from_utf8(unconfirmed_exp.stderr)
            .unwrap()
            .contains("explicit consent")
    );

    // Unconfirmed with --ceiling-bytes fails with explicit consent error
    let unconfirmed_ceil = workspace.run(&[
        "model",
        "load-probe",
        "dociler-lite",
        "--ceiling-bytes",
        "4000000000",
    ]);
    assert_eq!(unconfirmed_ceil.status.code(), Some(1));
    assert!(
        String::from_utf8(unconfirmed_ceil.stderr)
            .unwrap()
            .contains("explicit consent")
    );

    // Invalid ceiling values return exit code 2 (unrecognized command / invalid syntax)
    for bad_args in [
        vec![
            "model",
            "load-probe",
            "dociler-lite",
            "--confirm",
            "--ceiling-bytes",
            "0",
        ],
        vec![
            "model",
            "load-probe",
            "dociler-lite",
            "--confirm",
            "--ceiling-bytes",
            "not-a-number",
        ],
        vec![
            "model",
            "load-probe",
            "dociler-lite",
            "--confirm",
            "--ceiling-bytes=0",
        ],
        vec![
            "model",
            "load-probe",
            "dociler-lite",
            "--confirm",
            "--ceiling-bytes",
        ],
        vec![
            "model",
            "load-probe",
            "dociler-lite",
            "--confirm",
            "--unknown-flag",
        ],
    ] {
        let bad = workspace.run(&bad_args);
        assert_eq!(bad.status.code(), Some(2));
    }

    // Valid flags are recognized; execution proceeds past argument parsing to missing GGUF check (code 1)
    let valid_exp = workspace.run(&[
        "model",
        "load-probe",
        "dociler-lite",
        "--confirm",
        "--experimental",
    ]);
    assert_eq!(valid_exp.status.code(), Some(1));
    assert!(
        String::from_utf8(valid_exp.stderr)
            .unwrap()
            .contains("GGUF is missing")
    );

    let valid_ceil = workspace.run(&[
        "model",
        "load-probe",
        "dociler-lite",
        "--confirm",
        "--ceiling-bytes",
        "4000000000",
    ]);
    assert_eq!(valid_ceil.status.code(), Some(1));
    assert!(
        String::from_utf8(valid_ceil.stderr)
            .unwrap()
            .contains("GGUF is missing")
    );

    let valid_inline_ceil = workspace.run(&[
        "model",
        "load-probe",
        "dociler-lite",
        "--confirm",
        "--ceiling-bytes=4000000000",
    ]);
    assert_eq!(valid_inline_ceil.status.code(), Some(1));
    assert!(
        String::from_utf8(valid_inline_ceil.stderr)
            .unwrap()
            .contains("GGUF is missing")
    );

    // Arbitrary flag ordering and aliases (--allow-experimental, --memory-ceiling-bytes)
    let valid_all = workspace.run(&[
        "model",
        "load-probe",
        "dociler-lite",
        "--allow-experimental",
        "--memory-ceiling-bytes=4000000000",
        "--confirm",
    ]);
    assert_eq!(valid_all.status.code(), Some(1));
    assert!(
        String::from_utf8(valid_all.stderr)
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

#[test]
fn model_remove_requires_consent_and_dry_runs_without_writes() {
    let workspace = Workspace::new();
    let unconfirmed = workspace.run(&["model", "remove", "dociler-lite"]);
    assert_eq!(unconfirmed.status.code(), Some(1));
    let stdout = String::from_utf8(unconfirmed.stdout).unwrap();
    let stderr = String::from_utf8(unconfirmed.stderr).unwrap();
    assert!(stdout.contains("Previewing cache removal for dociler-lite"));
    assert!(stderr.contains("explicit consent"));

    // Set up a mock cached file in data directory
    let data_dir = workspace.0.join("data");
    let model_file = data_dir.join("models/manifest-v1/dociler-lite/Qwen_Qwen3.5-4B-Q4_K_M.gguf");
    fs::create_dir_all(model_file.parent().unwrap()).unwrap();
    fs::write(&model_file, b"model-bytes").unwrap();
    assert!(model_file.exists());

    // Without confirm, preview shows candidate but does not delete
    let preview = workspace.run(&["model", "remove", "dociler-lite"]);
    assert_eq!(preview.status.code(), Some(1));
    assert!(model_file.exists());

    // With confirm, file is safely removed
    let confirmed = workspace.run(&["model", "remove", "dociler-lite", "--confirm"]);
    assert_eq!(confirmed.status.code(), Some(0));
    assert!(!model_file.exists());
    let confirmed_stdout = String::from_utf8(confirmed.stdout).unwrap();
    assert!(confirmed_stdout.contains("Removed 1 cached asset file(s) for dociler-lite"));
}

#[test]
fn model_repair_requires_consent_and_dry_runs_without_writes() {
    let workspace = Workspace::new();
    let unconfirmed = workspace.run(&["model", "repair", "dociler-lite"]);
    assert_eq!(unconfirmed.status.code(), Some(1));
    let stdout = String::from_utf8(unconfirmed.stdout).unwrap();
    let stderr = String::from_utf8(unconfirmed.stderr).unwrap();
    assert!(stdout.contains("Inspecting assets for dociler-lite"));
    assert!(stdout.contains("Repair required"));
    assert!(stderr.contains("explicit consent"));
}

#[test]
fn files_command_discovers_workspace_documents_and_respects_filters() {
    let workspace = Workspace::new();

    // 1. Empty workspace
    let empty_output = workspace.run(&["files"]);
    assert_eq!(empty_output.status.code(), Some(0));
    let stdout = String::from_utf8(empty_output.stdout).unwrap();
    assert!(stdout.contains("Workspace:"));
    assert!(stdout.contains("No supported documents discovered"));

    // 2. Add supported documents, gitignore, and unsupported files
    fs::write(workspace.0.join("README.md"), "# Hello Dociler").unwrap();
    fs::write(workspace.0.join("notes.txt"), "Meeting notes").unwrap();
    fs::write(workspace.0.join("draft.tmp.md"), "Temporary draft").unwrap();
    fs::write(workspace.0.join("program.exe"), "binary").unwrap();

    let gitignore = "*.tmp.md\n";
    fs::write(workspace.0.join(".gitignore"), gitignore).unwrap();

    let docs_dir = workspace.0.join("docs");
    fs::create_dir(&docs_dir).unwrap();
    fs::write(docs_dir.join("spec.docx"), "word document").unwrap();

    let output = workspace.run(&["files"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Discovered 3 document(s)"));
    assert!(stdout.contains("README.md (Markdown"));
    assert!(stdout.contains("[direct-editable]"));
    assert!(stdout.contains("notes.txt (Plain Text"));
    assert!(stdout.contains("docs/spec.docx (Word (.docx)"));
    assert!(!stdout.contains("program.exe"));
    assert!(!stdout.contains("draft.tmp.md"));
    assert!(stdout.contains("1 file(s) ignored by .gitignore or exclusions"));

    // 3. Explicit path argument: dociler files docs
    let sub_output = workspace.run(&["files", "docs"]);
    assert_eq!(sub_output.status.code(), Some(0));
    let sub_stdout = String::from_utf8(sub_output.stdout).unwrap();
    assert!(sub_stdout.contains("Discovered 1 document(s)"));
    assert!(sub_stdout.contains("spec.docx"));

    // 4. Invalid path argument
    let invalid_output = workspace.run(&["files", "nonexistent_dir"]);
    assert_eq!(invalid_output.status.code(), Some(1));
}

#[test]
fn worker_extract_subcommand_hidden_from_help() {
    let workspace = Workspace::new();
    let help_output = workspace.run(&["--help"]);
    assert_eq!(help_output.status.code(), Some(0));
    let stdout = String::from_utf8(help_output.stdout).unwrap();
    assert!(!stdout.contains("__worker-extract"));
}

#[test]
fn worker_extract_subcommand_plain_text_and_rtf() {
    let workspace = Workspace::new();

    // 1. Plain text extraction via worker
    let txt_path = workspace.0.join("sample.txt");
    fs::write(&txt_path, "Paragraph 1\n\nParagraph 2").unwrap();

    let output = workspace.run(&["__worker-extract", txt_path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("DOCILER_EXTRACT_V1\n"));
    assert!(stdout.contains("\"format\":\"PlainText\""));
    assert!(stdout.contains("Paragraph 1"));
    assert!(stdout.contains("Paragraph 2"));

    // 2. RTF extraction via worker
    let rtf_path = workspace.0.join("sample.rtf");
    let rtf_bytes =
        br#"{\rtf1\ansi\deff0 {\fonttbl{\f0 Arial;}} \b Bold Title\b0\par Regular body text.\par}"#;
    fs::write(&rtf_path, rtf_bytes).unwrap();

    let rtf_output = workspace.run(&["__worker-extract", rtf_path.to_str().unwrap()]);
    assert_eq!(rtf_output.status.code(), Some(0));
    let rtf_stdout = String::from_utf8(rtf_output.stdout).unwrap();
    assert!(rtf_stdout.starts_with("DOCILER_EXTRACT_V1\n"));
    assert!(rtf_stdout.contains("\"format\":\"Rtf\""));
    assert!(rtf_stdout.contains("Bold Title"));
    assert!(rtf_stdout.contains("Regular body text"));

    // 3. Sandboxed extraction API end-to-end using compiled dociler binary
    let limits = dociler_core::extractor::ExtractionLimits {
        worker_executable: Some(PathBuf::from(env!("CARGO_BIN_EXE_dociler"))),
        ..Default::default()
    };
    let doc = dociler_core::extractor::extract_document(&rtf_path, &limits, None)
        .expect("sandboxed extraction of RTF");
    assert_eq!(doc.blocks.len(), 2);
    assert_eq!(
        doc.source.format,
        dociler_core::document::DocumentFormat::Rtf
    );
}

#[test]
fn permissions_subcommand_inspect_grant_and_revoke() {
    let workspace = Workspace::new();

    // 1. Initial inspection: read-only
    let output = workspace.run(&["permissions"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Write grant: read-only"));
    assert!(stdout.contains("dociler permissions --grant"));

    // 2. Grant write access
    let grant_output = workspace.run(&["permissions", "--grant"]);
    assert_eq!(grant_output.status.code(), Some(0));
    let grant_stdout = String::from_utf8(grant_output.stdout).unwrap();
    assert!(grant_stdout.contains("Write grant: enabled"));
    assert!(grant_stdout.contains(
        "Each in-place mutation or export still requires explicit diff/preview confirmation"
    ));

    // 3. Inspect after grant
    let check_output = workspace.run(&["permissions"]);
    assert_eq!(check_output.status.code(), Some(0));
    let check_stdout = String::from_utf8(check_output.stdout).unwrap();
    assert!(check_stdout.contains("Write grant: enabled"));
    assert!(check_stdout.contains("dociler permissions --revoke"));

    // 4. Revoke write access
    let revoke_output = workspace.run(&["permissions", "--revoke"]);
    assert_eq!(revoke_output.status.code(), Some(0));
    let revoke_stdout = String::from_utf8(revoke_output.stdout).unwrap();
    assert!(revoke_stdout.contains("Write grant: revoked"));

    // 5. Inspect after revoke
    let final_output = workspace.run(&["permissions"]);
    assert_eq!(final_output.status.code(), Some(0));
    let final_stdout = String::from_utf8(final_output.stdout).unwrap();
    assert!(final_stdout.contains("Write grant: read-only"));
}

#[test]
fn export_subcommand_requires_permissions_preview_and_confirms() {
    let workspace = Workspace::new();
    let src_path = workspace.0.join("sample.txt");
    fs::write(
        &src_path,
        "Export test content paragraph 1.\n\nParagraph 2.",
    )
    .unwrap();
    let dest_rel = "exported.md";

    // 1. Export fails when read-only
    let denied_output = workspace.run(&["export", "md", dest_rel, "sample.txt", "--confirm"]);
    assert_eq!(denied_output.status.code(), Some(1));
    let stderr = String::from_utf8(denied_output.stderr).unwrap();
    assert!(stderr.contains("workspace write access is disabled"));

    // 2. Grant permissions
    let grant = workspace.run(&["permissions", "--grant"]);
    assert_eq!(grant.status.code(), Some(0));

    // 3. Export preview without --confirm
    let preview_output = workspace.run(&["export", "md", dest_rel, "sample.txt"]);
    assert_eq!(preview_output.status.code(), Some(1));
    let stdout = String::from_utf8(preview_output.stdout).unwrap();
    assert!(stdout.contains("Document export preview:"));
    assert!(stdout.contains("Target format: Markdown"));
    assert!(stdout.contains("Destination:   exported.md"));
    assert!(stdout.contains("Source file:   sample.txt"));
    assert!(stdout.contains("--confirm"));

    // Ensure dest was NOT written
    assert!(!workspace.0.join(dest_rel).exists());

    // 4. Export with --confirm
    let confirm_output = workspace.run(&["export", "md", dest_rel, "sample.txt", "--confirm"]);
    assert_eq!(confirm_output.status.code(), Some(0));
    let confirm_stdout = String::from_utf8(confirm_output.stdout).unwrap();
    assert!(confirm_stdout.contains("Exported document to"));

    // Ensure dest WAS written and contains content
    let exported_content = fs::read_to_string(workspace.0.join(dest_rel)).unwrap();
    assert!(exported_content.contains("Export test content paragraph 1."));
    assert!(exported_content.contains("Paragraph 2."));
}

#[test]
fn export_subcommand_from_stdin() {
    let workspace = Workspace::new();
    let grant = workspace.run(&["permissions", "--grant"]);
    assert_eq!(grant.status.code(), Some(0));

    let input = b"Hello from stdin streamed text!";
    let output = workspace.run_with_input(&["export", "txt", "out.txt", "--confirm"], input);
    assert_eq!(output.status.code(), Some(0));
    let dest = workspace.0.join("out.txt");
    assert!(dest.exists());
    let content = fs::read_to_string(dest).unwrap();
    assert!(content.contains("Hello from stdin streamed text!"));
}
