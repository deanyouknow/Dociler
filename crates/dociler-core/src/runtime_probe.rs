//! Bounded, model-free lifecycle probe for the pinned local `llama-server`.
//!
//! A successful probe proves only that the inventoried binary can start an
//! authenticated loopback router and stop cleanly. It does not admit a model,
//! GPU backend, or local chat session.

use std::collections::VecDeque;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde_json::Value;
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::assets::{LLAMA_CPP_BUILD, LLAMA_CPP_COMMIT, RuntimeAsset, VerificationLevel};
use crate::cancellation::CancellationToken;
use crate::paths::AppPaths;
use crate::runtime_install::{RuntimeInstallState, inspect_installed_runtime};

pub(crate) const DIAGNOSTIC_LIMIT: usize = 8 * 1024;
pub(crate) const RESPONSE_LIMIT: u64 = 8 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);
const SERVER_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const HTTP_TIMEOUT: Duration = Duration::from_millis(400);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeProbeOptions {
    pub confirmed: bool,
}

impl RuntimeProbeOptions {
    pub const fn confirmed() -> Self {
        Self { confirmed: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProbeError {
    ConsentRequired,
    InstallMissing,
    InstallInvalid,
    Cancelled,
    Randomness,
    Spawn,
    VersionMismatch,
    ProcessExited,
    TimedOut,
    InvalidResponse,
    AuthenticationFailed,
    StopFailed,
    Io(io::ErrorKind),
}

impl fmt::Display for RuntimeProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ConsentRequired => "runtime execution requires explicit consent",
            Self::InstallMissing => "the pinned runtime has not been installed",
            Self::InstallInvalid => "the installed runtime failed full inventory verification",
            Self::Cancelled => "runtime probe was cancelled",
            Self::Randomness => "an internal API key could not be generated",
            Self::Spawn => "the pinned runtime could not be started",
            Self::VersionMismatch => "the runtime build does not match the pinned version",
            Self::ProcessExited => "the runtime exited during its probe",
            Self::TimedOut => "the runtime did not become ready before the probe deadline",
            Self::InvalidResponse => "the runtime returned an unexpected probe response",
            Self::AuthenticationFailed => "the runtime did not enforce its internal API key",
            Self::StopFailed => "the runtime could not be cleanly terminated and reaped",
            Self::Io(_) => "runtime probe I/O failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RuntimeProbeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeProbeReport {
    pub build: &'static str,
    pub startup_ms: u128,
    pub diagnostic_bytes_seen: u64,
    pub diagnostic_bytes_retained: usize,
}

/// Execute the installed binary only after rehashing every inventoried file.
/// The process is never left running after this function returns.
pub fn probe_installed_runtime(
    paths: &AppPaths,
    runtime: RuntimeAsset,
    options: RuntimeProbeOptions,
    cancellation: &CancellationToken,
) -> Result<RuntimeProbeReport, RuntimeProbeError> {
    if !options.confirmed {
        return Err(RuntimeProbeError::ConsentRequired);
    }
    if cancellation.is_cancelled() {
        return Err(RuntimeProbeError::Cancelled);
    }
    let installed = inspect_installed_runtime(paths, runtime, VerificationLevel::Sha256);
    match installed.state() {
        RuntimeInstallState::Missing => return Err(RuntimeProbeError::InstallMissing),
        RuntimeInstallState::Verified => {}
        RuntimeInstallState::PresentUnverified | RuntimeInstallState::Invalid => {
            return Err(RuntimeProbeError::InstallInvalid);
        }
    }
    if cancellation.is_cancelled() {
        return Err(RuntimeProbeError::Cancelled);
    }
    probe_server(
        installed.server_path(),
        installed.path(),
        Some((paths, runtime)),
        cancellation,
    )
}

fn probe_server(
    executable: &Path,
    install_directory: &Path,
    revalidate: Option<(&AppPaths, RuntimeAsset)>,
    cancellation: &CancellationToken,
) -> Result<RuntimeProbeReport, RuntimeProbeError> {
    probe_server_with_timeout(
        executable,
        install_directory,
        revalidate,
        cancellation,
        SERVER_TIMEOUT,
    )
}

fn probe_server_with_timeout(
    executable: &Path,
    install_directory: &Path,
    revalidate: Option<(&AppPaths, RuntimeAsset)>,
    cancellation: &CancellationToken,
    server_timeout: Duration,
) -> Result<RuntimeProbeReport, RuntimeProbeError> {
    let probe_dir = tempfile::Builder::new()
        .prefix("dociler-runtime-probe-")
        .tempdir()
        .map_err(|error| RuntimeProbeError::Io(error.kind()))?;
    let cache_dir = probe_dir.path().join("cache");
    let models_dir = probe_dir.path().join("models");
    fs::create_dir(&cache_dir).map_err(|error| RuntimeProbeError::Io(error.kind()))?;
    fs::create_dir(&models_dir).map_err(|error| RuntimeProbeError::Io(error.kind()))?;

    let key = make_key()?;
    let key_file = probe_dir.path().join("internal-api-key");
    write_private_key(&key_file, &key)?;
    check_version(executable, install_directory, &probe_dir, cancellation)?;
    if cancellation.is_cancelled() {
        return Err(RuntimeProbeError::Cancelled);
    }
    if let Some((paths, runtime)) = revalidate {
        // The version check is itself a short-lived child. Close its gap to
        // the actual listening process with a second full inventory pass.
        if inspect_installed_runtime(paths, runtime, VerificationLevel::Sha256).state()
            != RuntimeInstallState::Verified
        {
            return Err(RuntimeProbeError::InstallInvalid);
        }
        if cancellation.is_cancelled() {
            return Err(RuntimeProbeError::Cancelled);
        }
    }

    let reservation = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| RuntimeProbeError::Io(error.kind()))?;
    let port = reservation
        .local_addr()
        .map_err(|error| RuntimeProbeError::Io(error.kind()))?
        .port();
    // llama-server does not inherit a listening socket. Release the OS-assigned
    // loopback port immediately before spawn, then authenticate the occupant.
    drop(reservation);
    let mut command = isolated_command(executable, install_directory, &probe_dir);
    command
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--api-key-file")
        .arg(&key_file)
        .arg("--models-dir")
        .arg(&models_dir)
        .arg("--no-models-autoload")
        .arg("--models-max")
        .arg("1")
        .arg("--parallel")
        .arg("1")
        .arg("--threads")
        .arg(safe_threads().to_string())
        .arg("--threads-http")
        .arg("1")
        .arg("--no-webui")
        .arg("--no-cors-credentials")
        .arg("--cors-origins")
        .arg("localhost")
        .arg("--no-slots");
    let start = Instant::now();
    let mut child = ManagedChild::spawn(&mut command)?;
    let capture = DiagnosticCapture::start(&mut child.child);
    let result = check_server(port, &key, &mut child, cancellation, start, server_timeout);
    let stopped = child.stop();
    let diagnostics = capture.finish();
    stopped?;
    result?;
    Ok(RuntimeProbeReport {
        build: LLAMA_CPP_BUILD,
        startup_ms: start.elapsed().as_millis(),
        diagnostic_bytes_seen: diagnostics.total,
        diagnostic_bytes_retained: diagnostics.bytes.len(),
    })
}

pub(crate) fn isolated_command(
    executable: &Path,
    install_directory: &Path,
    probe_dir: &TempDir,
) -> Command {
    let mut command = Command::new(executable);
    command
        .env_clear()
        .current_dir(install_directory)
        .env("LLAMA_CACHE", probe_dir.path().join("cache"))
        .env("HOME", probe_dir.path())
        .env("TMPDIR", probe_dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", install_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", install_directory);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.env("PATH", "/usr/bin:/bin");
        // A terminal Ctrl+C targets its foreground process group. Keep the
        // sidecar outside Dociler's group so the CLI can request cancellation
        // and confirm child cleanup instead of racing its signal exit.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        // Windows needs SystemRoot to locate system DLLs even with a cleared
        // environment. No inherited LLAMA_*, proxy, or model variable survives.
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command.env("PATH", install_directory);
    }
    command
}

fn safe_threads() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().saturating_sub(1).clamp(1, 4))
        .unwrap_or(1)
}

pub(crate) fn make_key() -> Result<Zeroizing<String>, RuntimeProbeError> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut *bytes).map_err(|_| RuntimeProbeError::Randomness)?;
    let mut value = Zeroizing::new(String::with_capacity(64));
    for byte in bytes.iter() {
        use std::fmt::Write as _;
        write!(&mut *value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(value)
}

pub(crate) fn write_private_key(path: &Path, key: &str) -> Result<(), RuntimeProbeError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| RuntimeProbeError::Io(error.kind()))?;
    file.write_all(key.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| RuntimeProbeError::Io(error.kind()))
}

pub(crate) fn check_version(
    executable: &Path,
    install_directory: &Path,
    probe_dir: &TempDir,
    cancellation: &CancellationToken,
) -> Result<(), RuntimeProbeError> {
    let mut command = isolated_command(executable, install_directory, probe_dir);
    command.arg("--version");
    let mut child = ManagedChild::spawn(&mut command)?;
    let capture = DiagnosticCapture::start(&mut child.child);
    let result = child.wait_until(Instant::now() + VERSION_TIMEOUT, cancellation);
    if result.is_err() {
        let _ = child.stop();
    }
    let log = capture.finish();
    let status = result?;
    if !status.success() {
        return Err(RuntimeProbeError::VersionMismatch);
    }
    let output = String::from_utf8_lossy(&log.bytes.into_iter().collect::<Vec<_>>()).into_owned();
    let build = LLAMA_CPP_BUILD.trim_start_matches('b');
    if !output.contains(&format!("build {build}, commit {}", &LLAMA_CPP_COMMIT[..7])) {
        return Err(RuntimeProbeError::VersionMismatch);
    }
    Ok(())
}

fn check_server(
    port: u16,
    key: &str,
    child: &mut ManagedChild,
    cancellation: &CancellationToken,
    start: Instant,
    server_timeout: Duration,
) -> Result<(), RuntimeProbeError> {
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| RuntimeProbeError::InvalidResponse)?;
    let base = format!("http://127.0.0.1:{port}");
    let deadline = start + server_timeout;
    loop {
        if cancellation.is_cancelled() {
            return Err(RuntimeProbeError::Cancelled);
        }
        if child
            .child
            .try_wait()
            .map_err(|error| RuntimeProbeError::Io(error.kind()))?
            .is_some()
        {
            return Err(RuntimeProbeError::ProcessExited);
        }
        if Instant::now() >= deadline {
            return Err(RuntimeProbeError::TimedOut);
        }
        match fetch_json(&client, &format!("{base}/health"), None) {
            Ok(Some((200, value))) if value["status"] == "ok" => break,
            Ok(Some((200, _))) => return Err(RuntimeProbeError::InvalidResponse),
            Ok(Some((503, _))) | Ok(None) => {}
            Ok(Some(_)) => return Err(RuntimeProbeError::InvalidResponse),
            Err(error) => return Err(error),
        }
        thread::sleep(POLL_INTERVAL);
    }
    if cancellation.is_cancelled() {
        return Err(RuntimeProbeError::Cancelled);
    }
    if !matches!(
        fetch_json(&client, &format!("{base}/v1/models"), None)?,
        Some((401, _))
    ) {
        return Err(RuntimeProbeError::AuthenticationFailed);
    }
    let models = fetch_json(&client, &format!("{base}/v1/models"), Some(key))?
        .ok_or(RuntimeProbeError::InvalidResponse)?;
    if models.0 != 200
        || models.1["data"]
            .as_array()
            .is_none_or(|data| !data.is_empty())
    {
        return Err(RuntimeProbeError::InvalidResponse);
    }
    let props = fetch_json(&client, &format!("{base}/props"), Some(key))?
        .ok_or(RuntimeProbeError::InvalidResponse)?;
    let build_info = props.1["build_info"].as_str().unwrap_or("");
    if props.0 != 200
        || props.1["role"] != "router"
        || props.1["model_path"] != "none"
        || props.1["models_autoload"] != false
        || props.1["max_instances"] != 1
        || !build_info.contains(LLAMA_CPP_BUILD)
        || !build_info.contains(&LLAMA_CPP_COMMIT[..7])
    {
        return Err(RuntimeProbeError::VersionMismatch);
    }
    if cancellation.is_cancelled() {
        return Err(RuntimeProbeError::Cancelled);
    }
    if child
        .child
        .try_wait()
        .map_err(|error| RuntimeProbeError::Io(error.kind()))?
        .is_some()
    {
        return Err(RuntimeProbeError::ProcessExited);
    }
    Ok(())
}

pub(crate) fn fetch_json(
    client: &Client,
    url: &str,
    key: Option<&str>,
) -> Result<Option<(u16, Value)>, RuntimeProbeError> {
    let mut request = client.get(url);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = match request.send() {
        Ok(response) => response,
        Err(error) if error.is_connect() || error.is_timeout() => return Ok(None),
        Err(_) => return Err(RuntimeProbeError::InvalidResponse),
    };
    let status = response.status().as_u16();
    let mut body = Vec::new();
    response
        .take(RESPONSE_LIMIT + 1)
        .read_to_end(&mut body)
        .map_err(|_| RuntimeProbeError::InvalidResponse)?;
    if body.len() as u64 > RESPONSE_LIMIT {
        return Err(RuntimeProbeError::InvalidResponse);
    }
    let value = serde_json::from_slice(&body).map_err(|_| RuntimeProbeError::InvalidResponse)?;
    Ok(Some((status, value)))
}

pub(crate) struct ManagedChild {
    pub(crate) child: Child,
}

impl ManagedChild {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self, RuntimeProbeError> {
        command
            .spawn()
            .map(|child| Self { child })
            .map_err(|_| RuntimeProbeError::Spawn)
    }

    fn wait_until(
        &mut self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ExitStatus, RuntimeProbeError> {
        loop {
            if cancellation.is_cancelled() {
                return Err(RuntimeProbeError::Cancelled);
            }
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| RuntimeProbeError::Io(error.kind()))?
            {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(RuntimeProbeError::TimedOut);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    pub(crate) fn stop(&mut self) -> Result<(), RuntimeProbeError> {
        match self.child.try_wait() {
            Ok(Some(_)) => return Err(RuntimeProbeError::ProcessExited),
            Ok(None) => {}
            Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(RuntimeProbeError::StopFailed);
            }
        }
        let killed = self.child.kill();
        let reaped = self.child.wait();
        if killed.is_err() || reaped.is_err() {
            Err(RuntimeProbeError::StopFailed)
        } else {
            Ok(())
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) struct DiagnosticCapture {
    data: Arc<Mutex<BoundedLog>>,
    readers: Vec<thread::JoinHandle<()>>,
}

#[derive(Default)]
pub(crate) struct BoundedLog {
    pub(crate) bytes: VecDeque<u8>,
    pub(crate) total: u64,
}

impl DiagnosticCapture {
    pub(crate) fn start(child: &mut Child) -> Self {
        let data = Arc::new(Mutex::new(BoundedLog::default()));
        let mut readers = Vec::new();
        for stream in [
            child.stdout.take().map(Stream::Stdout),
            child.stderr.take().map(Stream::Stderr),
        ]
        .into_iter()
        .flatten()
        {
            let data = Arc::clone(&data);
            readers.push(thread::spawn(move || {
                let mut input: Box<dyn Read + Send> = match stream {
                    Stream::Stdout(stream) => Box::new(stream),
                    Stream::Stderr(stream) => Box::new(stream),
                };
                let mut buffer = [0_u8; 1024];
                while let Ok(count) = input.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    if let Ok(mut log) = data.lock() {
                        log.push(&buffer[..count]);
                    }
                }
            }));
        }
        Self { data, readers }
    }

    pub(crate) fn finish(self) -> BoundedLog {
        let deadline = Instant::now() + Duration::from_secs(1);
        for reader in self.readers {
            while !reader.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if reader.is_finished() {
                let _ = reader.join();
            }
            // A descendant retaining a pipe must not make shutdown unbounded.
            // Dropping an unfinished handle detaches its bounded reader.
        }
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *data)
    }
}

enum Stream {
    Stdout(std::process::ChildStdout),
    Stderr(std::process::ChildStderr),
}

impl BoundedLog {
    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        for byte in bytes {
            if self.bytes.len() == DIAGNOSTIC_LIMIT {
                self.bytes.pop_front();
            }
            self.bytes.push_back(*byte);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::assets::{AssetKind, AssetSpec};
    use sha2::{Digest, Sha256};

    const MOCK_SERVER: &str = r#"#!/usr/bin/env python3
import http.server
import json
import os
import sys
import time

MODE = "__MODE__"
EXECUTED = __EXECUTED__
PID_PATH = __PID_PATH__
PGID_PATH = __PGID_PATH__
open(EXECUTED, "w", encoding="ascii").write("yes")
if "--version" in sys.argv:
    if MODE == "tamper_after_version":
        open(__file__, "a", encoding="ascii").write("changed after version check\n")
    if MODE == "bad_version":
        print("version: 0.4.0-dev (build 1, commit deadbee)")
    else:
        print("version: 0.4.0-dev (build 10809, commit 5266f24)")
    sys.exit(0)

open(PID_PATH, "w", encoding="ascii").write(str(os.getpid()))
open(PGID_PATH, "w", encoding="ascii").write(str(os.getpgrp()))
if MODE == "exit":
    sys.exit(7)
if MODE == "hang":
    time.sleep(30)
    sys.exit(0)
if any(arg in sys.argv for arg in ("--model", "-m", "-hf")):
    sys.exit(8)
def arg(name):
    return sys.argv[sys.argv.index(name) + 1]
port = int(arg("--port"))
key = open(arg("--api-key-file"), encoding="ascii").read().strip()
if MODE == "flood":
    sys.stderr.write("x" * 100000)
    sys.stderr.flush()

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        authorized = self.headers.get("Authorization") == "Bearer " + key
        if self.path == "/health":
            status, payload = 200, {"status": "bad" if MODE == "bad_health" else "ok"}
        elif not authorized and MODE != "no_auth":
            status, payload = 401, {"error": "unauthorized"}
        elif self.path == "/v1/models":
            status, payload = 200, {"data": []}
        elif self.path == "/props":
            status, payload = 200, {
                "role": "router", "model_path": "none", "models_autoload": False,
                "max_instances": 1,
                "build_info": "b1-deadbee" if MODE == "bad_build" else "b10809-5266f24",
            }
        else:
            status, payload = 404, {"error": "not found"}
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_):
        pass

http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
"#;

    struct Fixture {
        _temp: TempDir,
        paths: AppPaths,
        runtime: RuntimeAsset,
        server: std::path::PathBuf,
        executed: std::path::PathBuf,
        pid_path: std::path::PathBuf,
        pgid_path: std::path::PathBuf,
    }

    impl Fixture {
        fn new(mode: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let paths = AppPaths::new(
                temp.path().join("config"),
                temp.path().join("data"),
                temp.path().join("cache"),
            )
            .unwrap();
            let runtime = RuntimeAsset::test_fixture(
                "linux",
                "x86_64",
                AssetSpec::test_fixture(
                    AssetKind::RuntimeArchive,
                    "probe-test",
                    "mock.tar.gz",
                    1,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                ),
            );
            let root = runtime
                .artifact()
                .cache_path(&paths)
                .parent()
                .unwrap()
                .join("runtime");
            fs::create_dir_all(&root).unwrap();
            let server = root.join("llama-server");
            let executed = temp.path().join("executed");
            let pid_path = temp.path().join("child.pid");
            let pgid_path = temp.path().join("child.pgid");
            let script = MOCK_SERVER
                .replace("__MODE__", mode)
                .replace(
                    "__EXECUTED__",
                    &format!("{:?}", executed.display().to_string()),
                )
                .replace(
                    "__PID_PATH__",
                    &format!("{:?}", pid_path.display().to_string()),
                )
                .replace(
                    "__PGID_PATH__",
                    &format!("{:?}", pgid_path.display().to_string()),
                );
            fs::write(&server, &script).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&server, fs::Permissions::from_mode(0o700)).unwrap();
            let inventory = serde_json::json!({
                "schema": 1,
                "manifest_id": crate::assets::MANIFEST_ID,
                "asset_id": runtime.artifact().id(),
                "archive_sha256": runtime.artifact().sha256(),
                "server_file": "llama-server",
                "files": [{
                    "path": "llama-server",
                    "bytes": script.len(),
                    "sha256": format!("{:x}", Sha256::digest(script.as_bytes())),
                }],
            });
            fs::write(
                root.join(".dociler-runtime.json"),
                serde_json::to_vec(&inventory).unwrap(),
            )
            .unwrap();
            Self {
                _temp: temp,
                paths,
                runtime,
                server,
                executed,
                pid_path,
                pgid_path,
            }
        }

        fn assert_child_reaped(&self) {
            let pid = fs::read_to_string(&self.pid_path).unwrap();
            assert!(!Path::new("/proc").join(pid).exists());
        }
    }

    #[test]
    fn consent_and_full_inventory_precede_any_execution() {
        let fixture = Fixture::new("healthy");
        let cancellation = CancellationToken::new();
        assert_eq!(
            probe_installed_runtime(
                &fixture.paths,
                fixture.runtime,
                RuntimeProbeOptions { confirmed: false },
                &cancellation,
            ),
            Err(RuntimeProbeError::ConsentRequired),
        );
        assert!(!fixture.executed.exists());
        fs::write(&fixture.server, "tampered").unwrap();
        assert_eq!(
            probe_installed_runtime(
                &fixture.paths,
                fixture.runtime,
                RuntimeProbeOptions::confirmed(),
                &cancellation,
            ),
            Err(RuntimeProbeError::InstallInvalid),
        );
        assert!(!fixture.executed.exists());
    }

    #[test]
    fn healthy_model_free_probe_reaps_child_and_bounds_diagnostics() {
        let fixture = Fixture::new("flood");
        let report = probe_installed_runtime(
            &fixture.paths,
            fixture.runtime,
            RuntimeProbeOptions::confirmed(),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(report.build, LLAMA_CPP_BUILD);
        assert!(report.diagnostic_bytes_seen >= 100_000);
        assert!(report.diagnostic_bytes_retained <= DIAGNOSTIC_LIMIT);
        assert_eq!(
            fs::read_to_string(&fixture.pid_path).unwrap(),
            fs::read_to_string(&fixture.pgid_path).unwrap(),
            "the Unix sidecar should lead its own process group"
        );
        fixture.assert_child_reaped();
    }

    #[test]
    fn cancellation_stops_a_hung_child() {
        let fixture = Fixture::new("hang");
        let cancellation = CancellationToken::new();
        let signal = cancellation.clone();
        let pid_path = fixture.pid_path.clone();
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !pid_path.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            signal.cancel();
        });
        let start = Instant::now();
        let result = probe_installed_runtime(
            &fixture.paths,
            fixture.runtime,
            RuntimeProbeOptions::confirmed(),
            &cancellation,
        );
        handle.join().unwrap();
        assert_eq!(result, Err(RuntimeProbeError::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(3));
        fixture.assert_child_reaped();
    }

    #[test]
    fn timeout_and_authentication_failure_stop_the_child() {
        let timed_out = Fixture::new("hang");
        assert_eq!(
            probe_server_with_timeout(
                &timed_out.server,
                timed_out.server.parent().unwrap(),
                None,
                &CancellationToken::new(),
                Duration::from_millis(200),
            ),
            Err(RuntimeProbeError::TimedOut),
        );
        timed_out.assert_child_reaped();

        let unsecured = Fixture::new("no_auth");
        assert_eq!(
            probe_installed_runtime(
                &unsecured.paths,
                unsecured.runtime,
                RuntimeProbeOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeProbeError::AuthenticationFailed),
        );
        unsecured.assert_child_reaped();
    }

    #[test]
    fn wrong_version_and_unexpected_health_fail_closed() {
        let version = Fixture::new("bad_version");
        assert_eq!(
            probe_installed_runtime(
                &version.paths,
                version.runtime,
                RuntimeProbeOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeProbeError::VersionMismatch),
        );
        assert!(!version.pid_path.exists());

        let tampered = Fixture::new("tamper_after_version");
        assert_eq!(
            probe_installed_runtime(
                &tampered.paths,
                tampered.runtime,
                RuntimeProbeOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeProbeError::InstallInvalid),
        );
        assert!(!tampered.pid_path.exists());

        let build = Fixture::new("bad_build");
        assert_eq!(
            probe_installed_runtime(
                &build.paths,
                build.runtime,
                RuntimeProbeOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeProbeError::VersionMismatch),
        );
        build.assert_child_reaped();

        let health = Fixture::new("bad_health");
        assert_eq!(
            probe_installed_runtime(
                &health.paths,
                health.runtime,
                RuntimeProbeOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeProbeError::InvalidResponse),
        );
        health.assert_child_reaped();

        let exited = Fixture::new("exit");
        assert_eq!(
            probe_installed_runtime(
                &exited.paths,
                exited.runtime,
                RuntimeProbeOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeProbeError::ProcessExited),
        );
        exited.assert_child_reaped();
    }
}
