//! Diagnostic-only local model load and tiny generation admission probe.
//!
//! A pass proves neither the target context/memory budget nor model quality.
//! The child is never retained for chat or exposed as a Dociler API backend.

use std::fmt;
use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::blocking::Client as BlockingClient;
use reqwest::redirect::Policy;
use serde_json::{Value, json};

use crate::assets::{
    AssetSpec, CacheState, LLAMA_CPP_BUILD, LLAMA_CPP_COMMIT, RuntimeAsset, VerificationLevel,
    inspect_cached_asset_cancellable, model_asset,
};
use crate::cancellation::CancellationToken;
use crate::config::LocalProfile;
use crate::hardware::{HardwareInventory, ModelPreflight, PreflightStatus};
use crate::paths::AppPaths;
use crate::probe_memory::ProcessGroupMemorySampler;
use crate::runtime_install::{RuntimeInstallState, inspect_installed_runtime_cancellable};
use crate::runtime_probe::{
    DiagnosticCapture, HTTP_TIMEOUT, ManagedChild, POLL_INTERVAL, RuntimeProbeError, check_version,
    fetch_json, isolated_command, make_key, write_private_key,
};

const LOAD_TIMEOUT: Duration = Duration::from_secs(180);
const GENERATION_TIMEOUT: Duration = Duration::from_secs(45);
const GENERATION_RESPONSE_LIMIT: usize = 16 * 1024;
const DIAGNOSTIC_CONTEXT: u32 = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelProbeError {
    ConsentRequired,
    Cancelled,
    ModelMissing,
    ModelInvalid,
    RuntimeMissing,
    RuntimeInvalid,
    UnsupportedCpuInstructions(Vec<&'static str>),
    HardwareNotReady(PreflightStatus),
    Runtime(RuntimeProbeError),
    ModelIdentity,
    Generation,
    TimedOut,
    MemoryLimitExceeded {
        limit_bytes: u64,
        observed_bytes: u64,
    },
}

impl fmt::Display for ModelProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ConsentRequired => "model execution requires explicit consent",
            Self::Cancelled => "model probe was cancelled",
            Self::ModelMissing => "the pinned model is not cached",
            Self::ModelInvalid => "the pinned model failed full verification",
            Self::RuntimeMissing => "the pinned runtime is not installed",
            Self::RuntimeInvalid => "the installed runtime failed full inventory verification",
            Self::UnsupportedCpuInstructions(missing) => {
                return write!(
                    formatter,
                    "the host CPU lacks required instructions for this runtime: {}",
                    missing.join(", ")
                );
            }
            Self::HardwareNotReady(_) => "live hardware preflight did not admit this model",
            Self::Runtime(_) => "the pinned runtime failed its model probe",
            Self::ModelIdentity => "the runtime reported an unexpected model or build",
            Self::Generation => "bounded test generation failed",
            Self::TimedOut => "model load or generation exceeded its deadline",
            Self::MemoryLimitExceeded {
                limit_bytes,
                observed_bytes,
            } => {
                return write!(
                    formatter,
                    "process-group memory ({observed_bytes} bytes) exceeded ceiling ({limit_bytes} bytes); process was aborted"
                );
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ModelProbeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelProbeOptions {
    pub confirmed: bool,
    pub allow_experimental: bool,
    pub memory_ceiling_bytes: Option<u64>,
}

impl ModelProbeOptions {
    pub fn confirmed() -> Self {
        Self {
            confirmed: true,
            allow_experimental: false,
            memory_ceiling_bytes: None,
        }
    }

    pub fn with_allow_experimental(mut self, allow: bool) -> Self {
        self.allow_experimental = allow;
        self
    }

    pub fn with_memory_ceiling(mut self, bytes: u64) -> Self {
        self.memory_ceiling_bytes = Some(bytes);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelProbeReport {
    pub profile: LocalProfile,
    pub context_tokens: u32,
    pub startup_ms: u128,
    pub generation_ms: u128,
    pub diagnostic_bytes_seen: u64,
    /// Linux child-process VmHWM only; not Dociler process-group peak RSS.
    pub server_peak_rss_bytes: Option<u64>,
    /// Linux combined process-group peak RSS (Dociler host + server sidecar concurrent peak).
    pub process_group_peak_rss_bytes: Option<u64>,
}

/// Rehash pinned inputs, repeat live hardware preflight, and briefly load and
/// generate with the selected GGUF. This is not local-chat admission.
pub fn probe_cached_model(
    paths: &AppPaths,
    runtime: RuntimeAsset,
    profile: LocalProfile,
    confirmed: bool,
    cancellation: &CancellationToken,
) -> Result<ModelProbeReport, ModelProbeError> {
    probe_cached_model_with_options(
        paths,
        runtime,
        profile,
        ModelProbeOptions {
            confirmed,
            allow_experimental: false,
            memory_ceiling_bytes: None,
        },
        cancellation,
    )
}

pub fn probe_cached_model_with_options(
    paths: &AppPaths,
    runtime: RuntimeAsset,
    profile: LocalProfile,
    options: ModelProbeOptions,
    cancellation: &CancellationToken,
) -> Result<ModelProbeReport, ModelProbeError> {
    if !options.confirmed {
        return Err(ModelProbeError::ConsentRequired);
    }
    if cancellation.is_cancelled() {
        return Err(ModelProbeError::Cancelled);
    }
    let hardware = HardwareInventory::inspect(&paths.data_dir);
    let cpu_check = hardware.evaluate_cpu_instructions(runtime);
    if !cpu_check.supported {
        return Err(ModelProbeError::UnsupportedCpuInstructions(
            cpu_check.missing_required,
        ));
    }
    let model = model_asset(profile).artifact();
    verify_model(paths, model, cancellation)?;
    if cancellation.is_cancelled() {
        return Err(ModelProbeError::Cancelled);
    }
    let installed = inspect_installed_runtime_cancellable(
        paths,
        runtime,
        VerificationLevel::Sha256,
        Some(cancellation),
    );
    match installed.state() {
        RuntimeInstallState::Cancelled => return Err(ModelProbeError::Cancelled),
        RuntimeInstallState::Missing => return Err(ModelProbeError::RuntimeMissing),
        RuntimeInstallState::Verified => {}
        RuntimeInstallState::PresentUnverified | RuntimeInstallState::Invalid => {
            return Err(ModelProbeError::RuntimeInvalid);
        }
    }
    if cancellation.is_cancelled() {
        return Err(ModelProbeError::Cancelled);
    }
    let preflight = ModelPreflight::evaluate(profile, &hardware);
    admit_hardware(preflight.status(), options.allow_experimental)?;
    let default_ceiling = preflight.requirements().required_available_memory_bytes();
    let memory_ceiling = options.memory_ceiling_bytes.or(Some(default_ceiling));
    let model_path = model.cache_path(paths);
    let revalidate = || {
        verify_model(paths, model, cancellation)?;
        if cancellation.is_cancelled() {
            return Err(ModelProbeError::Cancelled);
        }
        let installed = inspect_installed_runtime_cancellable(
            paths,
            runtime,
            VerificationLevel::Sha256,
            Some(cancellation),
        );
        match installed.state() {
            RuntimeInstallState::Cancelled => return Err(ModelProbeError::Cancelled),
            RuntimeInstallState::Verified => {}
            _ => return Err(ModelProbeError::RuntimeInvalid),
        }
        if cancellation.is_cancelled() {
            return Err(ModelProbeError::Cancelled);
        }
        let hardware = HardwareInventory::inspect(&paths.data_dir);
        let cpu_check = hardware.evaluate_cpu_instructions(runtime);
        if !cpu_check.supported {
            return Err(ModelProbeError::UnsupportedCpuInstructions(
                cpu_check.missing_required,
            ));
        }
        admit_hardware(
            ModelPreflight::evaluate(profile, &hardware).status(),
            options.allow_experimental,
        )
    };
    run_model_probe(
        ProbeTarget {
            executable: installed.server_path(),
            install_directory: installed.path(),
            model_path: &model_path,
            profile,
        },
        revalidate,
        memory_ceiling,
        cancellation,
        LOAD_TIMEOUT,
    )
}

struct ProbeTarget<'a> {
    executable: &'a Path,
    install_directory: &'a Path,
    model_path: &'a Path,
    profile: LocalProfile,
}

fn verify_model(
    paths: &AppPaths,
    model: AssetSpec,
    cancellation: &CancellationToken,
) -> Result<(), ModelProbeError> {
    match inspect_cached_asset_cancellable(
        paths,
        model,
        VerificationLevel::Sha256,
        Some(cancellation),
    )
    .state()
    {
        CacheState::Verified => Ok(()),
        CacheState::Cancelled => Err(ModelProbeError::Cancelled),
        CacheState::Missing => Err(ModelProbeError::ModelMissing),
        _ => Err(ModelProbeError::ModelInvalid),
    }
}

fn admit_hardware(
    status: PreflightStatus,
    allow_experimental: bool,
) -> Result<(), ModelProbeError> {
    match status {
        PreflightStatus::ReadyForRuntimeProbe => Ok(()),
        PreflightStatus::ExperimentalForRuntimeProbe if allow_experimental => Ok(()),
        // Experimental 6–8 GB use requires its own measured load gate. A
        // positive planning preflight alone cannot silently enable it.
        _ => Err(ModelProbeError::HardwareNotReady(status)),
    }
}

fn run_model_probe<F>(
    target: ProbeTarget<'_>,
    revalidate: F,
    memory_ceiling_bytes: Option<u64>,
    cancellation: &CancellationToken,
    load_timeout: Duration,
) -> Result<ModelProbeReport, ModelProbeError>
where
    F: FnOnce() -> Result<(), ModelProbeError>,
{
    let probe_dir = tempfile::Builder::new()
        .prefix("dociler-model-probe-")
        .tempdir()
        .map_err(|error| ModelProbeError::Runtime(RuntimeProbeError::Io(error.kind())))?;
    fs::create_dir(probe_dir.path().join("cache"))
        .map_err(|error| ModelProbeError::Runtime(RuntimeProbeError::Io(error.kind())))?;
    let key = make_key().map_err(ModelProbeError::Runtime)?;
    let key_file = probe_dir.path().join("internal-api-key");
    write_private_key(&key_file, &key).map_err(ModelProbeError::Runtime)?;
    check_version(
        target.executable,
        target.install_directory,
        &probe_dir,
        cancellation,
    )
    .map_err(ModelProbeError::Runtime)?;
    if cancellation.is_cancelled() {
        return Err(ModelProbeError::Cancelled);
    }
    revalidate()?;

    let reservation = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| ModelProbeError::Runtime(RuntimeProbeError::Io(error.kind())))?;
    let port = reservation
        .local_addr()
        .map_err(|error| ModelProbeError::Runtime(RuntimeProbeError::Io(error.kind())))?
        .port();
    drop(reservation);
    let mut command = isolated_command(target.executable, target.install_directory, &probe_dir);
    command
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--api-key-file")
        .arg(&key_file)
        .arg("--model")
        .arg(target.model_path)
        .arg("--alias")
        .arg(target.profile.alias())
        .arg("--ctx-size")
        .arg(DIAGNOSTIC_CONTEXT.to_string())
        .arg("--parallel")
        .arg("1")
        .arg("--threads")
        .arg(safe_threads().to_string())
        .arg("--threads-http")
        .arg("1")
        .arg("--batch-size")
        .arg("128")
        .arg("--ubatch-size")
        .arg("64")
        .arg("--n-gpu-layers")
        .arg("0")
        .arg("--fit")
        .arg("off")
        .arg("--flash-attn")
        .arg("off")
        .arg("--no-context-shift")
        .arg("--no-mmproj")
        .arg("--no-webui")
        .arg("--no-slots")
        .arg("--no-cors-credentials")
        .arg("--cors-origins")
        .arg("localhost");

    let start = Instant::now();
    let mut child = ManagedChild::spawn(&mut command).map_err(ModelProbeError::Runtime)?;
    let memory = ProcessGroupMemorySampler::start(
        std::process::id(),
        child.child.id(),
        memory_ceiling_bytes,
        cancellation.clone(),
    );
    let capture = DiagnosticCapture::start(&mut child.child);
    let result = check_loaded_server(
        AuthenticatedEndpoint { port, key: &key },
        target.model_path,
        target.profile,
        &mut child,
        cancellation,
        start,
        load_timeout,
    );
    let abort_info = memory.abort_info();
    let memory_report = memory.finish();
    let stopped = child.stop();
    let diagnostics = capture.finish();
    stopped.map_err(ModelProbeError::Runtime)?;
    if let Some(abort) = abort_info {
        return Err(ModelProbeError::MemoryLimitExceeded {
            limit_bytes: abort.limit_bytes,
            observed_bytes: abort.observed_bytes,
        });
    }
    let (startup_ms, generation_ms) = result?;
    Ok(ModelProbeReport {
        profile: target.profile,
        context_tokens: DIAGNOSTIC_CONTEXT,
        startup_ms,
        generation_ms,
        diagnostic_bytes_seen: diagnostics.total,
        server_peak_rss_bytes: memory_report.server_peak_rss_bytes,
        process_group_peak_rss_bytes: memory_report.process_group_peak_rss_bytes,
    })
}

fn safe_threads() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().saturating_sub(1).clamp(1, 4))
        .unwrap_or(1)
}

struct AuthenticatedEndpoint<'a> {
    port: u16,
    key: &'a str,
}

fn check_loaded_server(
    endpoint: AuthenticatedEndpoint<'_>,
    model_path: &Path,
    profile: LocalProfile,
    child: &mut ManagedChild,
    cancellation: &CancellationToken,
    start: Instant,
    load_timeout: Duration,
) -> Result<(u128, u128), ModelProbeError> {
    let client = BlockingClient::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|_| ModelProbeError::Runtime(RuntimeProbeError::InvalidResponse))?;
    let base = format!("http://127.0.0.1:{}", endpoint.port);
    loop {
        if cancellation.is_cancelled() {
            return Err(ModelProbeError::Cancelled);
        }
        if child
            .child
            .try_wait()
            .map_err(|error| ModelProbeError::Runtime(RuntimeProbeError::Io(error.kind())))?
            .is_some()
        {
            return Err(ModelProbeError::Runtime(RuntimeProbeError::ProcessExited));
        }
        if start.elapsed() >= load_timeout {
            return Err(ModelProbeError::TimedOut);
        }
        match fetch_json(&client, &format!("{base}/health"), None)
            .map_err(ModelProbeError::Runtime)?
        {
            Some((200, value)) if value["status"] == "ok" => break,
            Some((503, _)) | None => {}
            _ => return Err(ModelProbeError::Runtime(RuntimeProbeError::InvalidResponse)),
        }
        thread::sleep(POLL_INTERVAL);
    }
    if cancellation.is_cancelled() {
        return Err(ModelProbeError::Cancelled);
    }
    let startup_ms = start.elapsed().as_millis();
    if !matches!(
        fetch_json(&client, &format!("{base}/v1/models"), None)
            .map_err(ModelProbeError::Runtime)?,
        Some((401, _))
    ) {
        return Err(ModelProbeError::Runtime(
            RuntimeProbeError::AuthenticationFailed,
        ));
    }
    let models = fetch_json(&client, &format!("{base}/v1/models"), Some(endpoint.key))
        .map_err(ModelProbeError::Runtime)?
        .ok_or(ModelProbeError::ModelIdentity)?;
    let data = models.1["data"]
        .as_array()
        .ok_or(ModelProbeError::ModelIdentity)?;
    if models.0 != 200 || data.len() != 1 || data[0]["id"] != profile.alias() {
        return Err(ModelProbeError::ModelIdentity);
    }
    let props = fetch_json(&client, &format!("{base}/props"), Some(endpoint.key))
        .map_err(ModelProbeError::Runtime)?
        .ok_or(ModelProbeError::ModelIdentity)?;
    let expected_path = model_path.to_str().ok_or(ModelProbeError::ModelIdentity)?;
    let build = props.1["build_info"].as_str().unwrap_or("");
    if props.0 != 200
        || props.1["model_path"] != expected_path
        || !build.contains(LLAMA_CPP_BUILD)
        || !build.contains(&LLAMA_CPP_COMMIT[..7])
    {
        return Err(ModelProbeError::ModelIdentity);
    }
    let generation_start = Instant::now();
    generate_once(&base, endpoint.key, profile, cancellation)?;
    if cancellation.is_cancelled() {
        return Err(ModelProbeError::Cancelled);
    }
    if child
        .child
        .try_wait()
        .map_err(|error| ModelProbeError::Runtime(RuntimeProbeError::Io(error.kind())))?
        .is_some()
    {
        return Err(ModelProbeError::Runtime(RuntimeProbeError::ProcessExited));
    }
    Ok((startup_ms, generation_start.elapsed().as_millis()))
}

fn generate_once(
    base: &str,
    key: &str,
    profile: LocalProfile,
    cancellation: &CancellationToken,
) -> Result<(), ModelProbeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ModelProbeError::Generation)?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(GENERATION_TIMEOUT)
            .build()
            .map_err(|_| ModelProbeError::Generation)?;
        let payload = json!({
            "model": profile.alias(),
            "messages": [{"role": "user", "content": "Reply with one short word."}],
            "max_tokens": 16,
            "temperature": 0,
            "stream": false,
            "reasoning_effort": "none",
            "chat_template_kwargs": {"enable_thinking": false}
        });
        let request = client
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(key)
            .json(&payload)
            .send();
        let mut task = Box::pin(async {
            let response = request.await.map_err(|_| ModelProbeError::Generation)?;
            if response.status().as_u16() != 200
                || response
                    .content_length()
                    .is_some_and(|size| size > GENERATION_RESPONSE_LIMIT as u64)
            {
                return Err(ModelProbeError::Generation);
            }
            let mut body = Vec::new();
            let mut chunks = response.bytes_stream();
            while let Some(next) = chunks.next().await {
                let chunk = next.map_err(|_| ModelProbeError::Generation)?;
                if body.len().saturating_add(chunk.len()) > GENERATION_RESPONSE_LIMIT {
                    return Err(ModelProbeError::Generation);
                }
                body.extend_from_slice(&chunk);
            }
            let value: Value =
                serde_json::from_slice(&body).map_err(|_| ModelProbeError::Generation)?;
            let content = value["choices"][0]["message"]["content"]
                .as_str()
                .ok_or(ModelProbeError::Generation)?;
            if content.trim().is_empty() {
                return Err(ModelProbeError::Generation);
            }
            Ok(())
        });
        let deadline = Instant::now() + GENERATION_TIMEOUT;
        loop {
            if cancellation.is_cancelled() {
                return Err(ModelProbeError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(ModelProbeError::TimedOut);
            }
            if let Ok(result) = tokio::time::timeout(Duration::from_millis(25), &mut task).await {
                return result;
            }
        }
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::assets::{AssetKind, AssetSpec};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    const MOCK_SERVER: &str = r#"#!/usr/bin/env python3
import http.server
import json
import os
import sys
import time

MODE = "__MODE__"
PID = __PID__
ARGS = __ARGS__
REQUEST = __REQUEST__
if "--version" in sys.argv:
    print("version: 0.4.0-dev (build 10809, commit 5266f24)")
    sys.exit(0)

open(PID, "w", encoding="ascii").write(str(os.getpid()))
open(ARGS, "w", encoding="utf-8").write(json.dumps(sys.argv[1:]))
def arg(name):
    return sys.argv[sys.argv.index(name) + 1]
port = int(arg("--port"))
key = open(arg("--api-key-file"), encoding="ascii").read().strip()
model = arg("--model")
alias = arg("--alias")
if not os.path.isfile(model):
    sys.exit(9)

class Handler(http.server.BaseHTTPRequestHandler):
    def respond(self, status, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        authorized = self.headers.get("Authorization") == "Bearer " + key
        if self.path == "/health":
            self.respond(503 if MODE == "loading" else 200, {"status": "ok"})
        elif not authorized and MODE != "no_auth":
            self.respond(401, {"error": "unauthorized"})
        elif self.path == "/v1/models":
            self.respond(200, {"data": [{"id": "wrong" if MODE == "wrong_model" else alias}]})
        elif self.path == "/props":
            self.respond(200, {"model_path": model, "build_info": "b10809-5266f24"})
        else:
            self.respond(404, {"error": "not found"})
    def do_POST(self):
        if self.headers.get("Authorization") != "Bearer " + key:
            self.respond(401, {"error": "unauthorized"})
            return
        if self.path != "/v1/chat/completions":
            self.respond(404, {"error": "not found"})
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        open(REQUEST, "w", encoding="utf-8").write(json.dumps(body))
        if MODE == "hang_generation":
            time.sleep(30)
        elif MODE == "empty_generation":
            self.respond(200, {"choices": [{"message": {"content": ""}}]})
        elif MODE == "oversized_generation":
            self.respond(200, {"choices": [{"message": {"content": "x" * 20000}}]})
        else:
            self.respond(200, {"choices": [{"message": {"content": "OK"}}]})
    def log_message(self, *_):
        pass

http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
"#;

    struct Fixture {
        _temp: tempfile::TempDir,
        server: std::path::PathBuf,
        model: std::path::PathBuf,
        pid: std::path::PathBuf,
        args: std::path::PathBuf,
        request: std::path::PathBuf,
    }

    impl Fixture {
        fn new(mode: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("runtime");
            fs::create_dir(&root).unwrap();
            let server = root.join("llama-server");
            let model = temp.path().join("tiny fixture.gguf");
            let pid = temp.path().join("child.pid");
            let args = temp.path().join("args.json");
            let request = temp.path().join("request.json");
            fs::write(&model, b"test-only mock model").unwrap();
            let script = MOCK_SERVER
                .replace("__MODE__", mode)
                .replace("__PID__", &format!("{:?}", pid.display().to_string()))
                .replace("__ARGS__", &format!("{:?}", args.display().to_string()))
                .replace(
                    "__REQUEST__",
                    &format!("{:?}", request.display().to_string()),
                );
            fs::write(&server, script).unwrap();
            fs::set_permissions(&server, fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                _temp: temp,
                server,
                model,
                pid,
                args,
                request,
            }
        }

        fn run(
            &self,
            cancellation: &CancellationToken,
            timeout: Duration,
        ) -> Result<ModelProbeReport, ModelProbeError> {
            self.run_with_ceiling(None, cancellation, timeout)
        }

        fn run_with_ceiling(
            &self,
            memory_ceiling_bytes: Option<u64>,
            cancellation: &CancellationToken,
            timeout: Duration,
        ) -> Result<ModelProbeReport, ModelProbeError> {
            run_model_probe(
                ProbeTarget {
                    executable: &self.server,
                    install_directory: self.server.parent().unwrap(),
                    model_path: &self.model,
                    profile: LocalProfile::Lite,
                },
                || Ok(()),
                memory_ceiling_bytes,
                cancellation,
                timeout,
            )
        }

        fn assert_reaped(&self) {
            if let Ok(pid) = fs::read_to_string(&self.pid) {
                assert!(!Path::new("/proc").join(pid.trim()).exists());
            }
        }
    }

    #[test]
    fn hardware_gate_rejects_experimental_and_incomplete_states() {
        assert_eq!(
            admit_hardware(PreflightStatus::ReadyForRuntimeProbe, false),
            Ok(())
        );
        assert_eq!(
            admit_hardware(PreflightStatus::ReadyForRuntimeProbe, true),
            Ok(())
        );
        assert_eq!(
            admit_hardware(PreflightStatus::ExperimentalForRuntimeProbe, false),
            Err(ModelProbeError::HardwareNotReady(
                PreflightStatus::ExperimentalForRuntimeProbe
            ))
        );
        assert_eq!(
            admit_hardware(PreflightStatus::ExperimentalForRuntimeProbe, true),
            Ok(())
        );
        for status in [
            PreflightStatus::InsufficientTotalMemory,
            PreflightStatus::InsufficientAvailableMemory,
            PreflightStatus::InsufficientDisk,
            PreflightStatus::InventoryIncomplete,
        ] {
            assert_eq!(
                admit_hardware(status, false),
                Err(ModelProbeError::HardwareNotReady(status))
            );
            assert_eq!(
                admit_hardware(status, true),
                Err(ModelProbeError::HardwareNotReady(status))
            );
        }
    }

    #[test]
    fn consent_and_asset_hash_reject_before_any_execution() {
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
                "mock",
                "mock.tar.gz",
                1,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        );
        assert_eq!(
            probe_cached_model(
                &paths,
                runtime,
                LocalProfile::Lite,
                false,
                &CancellationToken::new()
            ),
            Err(ModelProbeError::ConsentRequired),
        );
        assert!(!paths.data_dir.exists());
        assert_eq!(
            probe_cached_model(
                &paths,
                runtime,
                LocalProfile::Lite,
                true,
                &CancellationToken::new()
            ),
            Err(ModelProbeError::ModelMissing),
        );
        assert!(!paths.data_dir.exists());

        let fixture = AssetSpec::test_fixture(
            AssetKind::Model,
            "test",
            "tiny.gguf",
            5,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        );
        let path = fixture.cache_path(&paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"hello").unwrap();
        assert_eq!(
            verify_model(&paths, fixture, &CancellationToken::new()),
            Ok(())
        );
        fs::write(&path, b"HELLO").unwrap();
        assert_eq!(
            verify_model(&paths, fixture, &CancellationToken::new()),
            Err(ModelProbeError::ModelInvalid)
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            verify_model(&paths, fixture, &cancelled),
            Err(ModelProbeError::Cancelled)
        );
    }

    #[test]
    fn probe_cached_model_aborts_when_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(
            temp.path().join("config"),
            temp.path().join("data"),
            temp.path().join("cache"),
        )
        .unwrap();
        let runtime = crate::assets::runtime_asset_for("linux", "x86_64").unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            probe_cached_model(&paths, *runtime, LocalProfile::Lite, true, &cancelled),
            Err(ModelProbeError::Cancelled)
        );
    }

    #[test]
    fn cpu_instruction_gate_rejects_missing_features_before_execution() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(
            temp.path().join("config"),
            temp.path().join("data"),
            temp.path().join("cache"),
        )
        .unwrap();
        let base_runtime = crate::assets::runtime_asset_for("linux", "x86_64").unwrap();
        let runtime_missing_instructions = RuntimeAsset::test_fixture_with_features(
            base_runtime.operating_system(),
            base_runtime.architecture(),
            base_runtime.artifact(),
            &["missing_cpu_feature_xyz"],
            &[],
            (1, 10),
        );
        let result = probe_cached_model_with_options(
            &paths,
            runtime_missing_instructions,
            LocalProfile::Lite,
            ModelProbeOptions {
                confirmed: true,
                allow_experimental: false,
                memory_ceiling_bytes: None,
            },
            &CancellationToken::new(),
        );
        assert_eq!(
            result,
            Err(ModelProbeError::UnsupportedCpuInstructions(vec![
                "missing_cpu_feature_xyz"
            ]))
        );
        assert!(!paths.data_dir.exists());
    }

    #[test]
    fn tiny_protocol_fixture_loads_generates_and_reaps() {
        let fixture = Fixture::new("healthy");
        let report = fixture
            .run(&CancellationToken::new(), Duration::from_secs(3))
            .unwrap();
        assert_eq!(report.profile, LocalProfile::Lite);
        assert_eq!(report.context_tokens, DIAGNOSTIC_CONTEXT);
        assert!(report.server_peak_rss_bytes.is_some_and(|bytes| bytes > 0));
        assert!(
            report
                .process_group_peak_rss_bytes
                .is_some_and(|bytes| bytes > 0)
        );
        fixture.assert_reaped();
        let args: Vec<String> = serde_json::from_slice(&fs::read(&fixture.args).unwrap()).unwrap();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", fixture.model.to_str().unwrap()])
        );
        assert!(args.windows(2).any(|pair| pair == ["--n-gpu-layers", "0"]));
        assert!(args.windows(2).any(|pair| pair == ["--parallel", "1"]));
        assert!(!args.iter().any(|arg| arg == "--api-key"));
        let request: Value = serde_json::from_slice(&fs::read(&fixture.request).unwrap()).unwrap();
        assert_eq!(request["model"], "dociler-lite");
        assert_eq!(request["max_tokens"], 16);
        assert_eq!(request["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn memory_ceiling_breach_aborts_probe_and_reaps_child() {
        let fixture = Fixture::new("healthy");
        let result =
            fixture.run_with_ceiling(Some(1), &CancellationToken::new(), Duration::from_secs(3));
        match result {
            Err(ModelProbeError::MemoryLimitExceeded {
                limit_bytes,
                observed_bytes,
            }) => {
                assert_eq!(limit_bytes, 1);
                assert!(observed_bytes > 1);
            }
            other => panic!("expected MemoryLimitExceeded, got {other:?}"),
        }
        fixture.assert_reaped();
    }

    #[test]
    fn post_version_revalidation_blocks_model_spawn() {
        let fixture = Fixture::new("healthy");
        let revalidated = AtomicBool::new(false);
        let result = run_model_probe(
            ProbeTarget {
                executable: &fixture.server,
                install_directory: fixture.server.parent().unwrap(),
                model_path: &fixture.model,
                profile: LocalProfile::Lite,
            },
            || {
                revalidated.store(true, Ordering::Release);
                Err(ModelProbeError::ModelInvalid)
            },
            None,
            &CancellationToken::new(),
            Duration::from_secs(3),
        );
        assert_eq!(result, Err(ModelProbeError::ModelInvalid));
        assert!(revalidated.load(Ordering::Acquire));
        assert!(!fixture.pid.exists());
    }

    #[test]
    fn unexpected_identity_auth_and_empty_generation_fail_closed() {
        for (mode, expected) in [
            ("wrong_model", ModelProbeError::ModelIdentity),
            (
                "no_auth",
                ModelProbeError::Runtime(RuntimeProbeError::AuthenticationFailed),
            ),
            ("empty_generation", ModelProbeError::Generation),
            ("oversized_generation", ModelProbeError::Generation),
        ] {
            let fixture = Fixture::new(mode);
            assert_eq!(
                fixture.run(&CancellationToken::new(), Duration::from_secs(3)),
                Err(expected)
            );
            fixture.assert_reaped();
        }
    }

    #[test]
    fn loading_timeout_and_generation_cancellation_reap_the_child() {
        let loading = Fixture::new("loading");
        assert_eq!(
            loading.run(&CancellationToken::new(), Duration::from_millis(250)),
            Err(ModelProbeError::TimedOut)
        );
        loading.assert_reaped();

        let stalled = Fixture::new("hang_generation");
        let cancellation = CancellationToken::new();
        let signal = cancellation.clone();
        let marker = stalled.request.clone();
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let observed = std::sync::Arc::clone(&cancelled);
        let notifier = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            observed.store(marker.exists(), Ordering::Release);
            signal.cancel();
        });
        let start = Instant::now();
        assert_eq!(
            stalled.run(&cancellation, Duration::from_secs(3)),
            Err(ModelProbeError::Cancelled)
        );
        notifier.join().unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(start.elapsed() < Duration::from_secs(4));
        stalled.assert_reaped();
    }

    #[test]
    fn loading_cancellation_reaps_the_child() {
        let loading = Fixture::new("loading");
        let cancellation = CancellationToken::new();
        let signal = cancellation.clone();
        let pid_file = loading.pid.clone();
        let notifier = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !pid_file.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            thread::sleep(Duration::from_millis(50));
            signal.cancel();
        });
        let start = Instant::now();
        assert_eq!(
            loading.run(&cancellation, Duration::from_secs(5)),
            Err(ModelProbeError::Cancelled)
        );
        notifier.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(4));
        loading.assert_reaped();
    }

    /// Opt-in protocol check against the real pinned binary and a small,
    /// independently licensed GGUF. This is not Lite hardware/quality admission.
    #[cfg(target_arch = "x86_64")]
    #[test]
    #[ignore = "requires the pinned Linux runtime and a separately downloaded 105 MB GGUF fixture"]
    fn real_smollm2_fixture_loads_and_generates() {
        let root = std::env::var_os("DOCILER_REAL_GGUF_TEST_ROOT")
            .expect("set DOCILER_REAL_GGUF_TEST_ROOT to the isolated fixture directory");
        let root = Path::new(&root);
        let paths =
            AppPaths::new(root.join("config"), root.join("data"), root.join("cache")).unwrap();
        let fixture = AssetSpec::test_fixture(
            AssetKind::Model,
            "real-smollm2",
            "SmolLM2-135M-Instruct-Q4_K_M.gguf",
            105_454_432,
            "2e8040ceae7815abe0dcb3540b9995eaa1fa0d2ca9e797d0a635ae4433c68c2d",
        );
        verify_model(&paths, fixture, &CancellationToken::new())
            .expect("real GGUF fixture must pass SHA-256 verification");
        let runtime = crate::assets::runtime_asset_for("linux", "x86_64").unwrap();
        let installed = inspect_installed_runtime_cancellable(
            &paths,
            *runtime,
            VerificationLevel::Sha256,
            None,
        );
        assert_eq!(installed.state(), RuntimeInstallState::Verified);
        let report = run_model_probe(
            ProbeTarget {
                executable: installed.server_path(),
                install_directory: installed.path(),
                model_path: &fixture.cache_path(&paths),
                profile: LocalProfile::Lite,
            },
            || {
                verify_model(&paths, fixture, &CancellationToken::new())?;
                if inspect_installed_runtime_cancellable(
                    &paths,
                    *runtime,
                    VerificationLevel::Sha256,
                    None,
                )
                .state()
                    != RuntimeInstallState::Verified
                {
                    return Err(ModelProbeError::RuntimeInvalid);
                }
                Ok(())
            },
            None,
            &CancellationToken::new(),
            LOAD_TIMEOUT,
        )
        .expect("the pinned server must load and generate with the real tiny fixture");
        assert_eq!(report.context_tokens, DIAGNOSTIC_CONTEXT);
        assert!(report.startup_ms < LOAD_TIMEOUT.as_millis());
        assert!(report.generation_ms < GENERATION_TIMEOUT.as_millis());
        assert!(report.server_peak_rss_bytes.is_some_and(|bytes| bytes > 0));
        assert!(
            report
                .process_group_peak_rss_bytes
                .is_some_and(|bytes| bytes > 0)
        );
        eprintln!(
            "diagnostic server-only peak RSS: {} bytes, process-group peak RSS: {} bytes",
            report.server_peak_rss_bytes.unwrap(),
            report.process_group_peak_rss_bytes.unwrap(),
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    #[ignore = "requires the pinned Linux runtime and a separately downloaded 105 MB GGUF fixture"]
    fn real_smollm2_fixture_aborts_on_memory_ceiling_breach() {
        let root = std::env::var_os("DOCILER_REAL_GGUF_TEST_ROOT")
            .expect("set DOCILER_REAL_GGUF_TEST_ROOT to the isolated fixture directory");
        let root = Path::new(&root);
        let paths =
            AppPaths::new(root.join("config"), root.join("data"), root.join("cache")).unwrap();
        let fixture = AssetSpec::test_fixture(
            AssetKind::Model,
            "real-smollm2",
            "SmolLM2-135M-Instruct-Q4_K_M.gguf",
            105_454_432,
            "2e8040ceae7815abe0dcb3540b9995eaa1fa0d2ca9e797d0a635ae4433c68c2d",
        );
        verify_model(&paths, fixture, &CancellationToken::new())
            .expect("real GGUF fixture must pass SHA-256 verification");
        let runtime = crate::assets::runtime_asset_for("linux", "x86_64").unwrap();
        let installed = inspect_installed_runtime_cancellable(
            &paths,
            *runtime,
            VerificationLevel::Sha256,
            None,
        );
        assert_eq!(installed.state(), RuntimeInstallState::Verified);
        let result = run_model_probe(
            ProbeTarget {
                executable: installed.server_path(),
                install_directory: installed.path(),
                model_path: &fixture.cache_path(&paths),
                profile: LocalProfile::Lite,
            },
            || {
                verify_model(&paths, fixture, &CancellationToken::new())?;
                if inspect_installed_runtime_cancellable(
                    &paths,
                    *runtime,
                    VerificationLevel::Sha256,
                    None,
                )
                .state()
                    != RuntimeInstallState::Verified
                {
                    return Err(ModelProbeError::RuntimeInvalid);
                }
                Ok(())
            },
            Some(1),
            &CancellationToken::new(),
            LOAD_TIMEOUT,
        );
        match result {
            Err(ModelProbeError::MemoryLimitExceeded {
                limit_bytes,
                observed_bytes,
            }) => {
                assert_eq!(limit_bytes, 1);
                assert!(observed_bytes > 1);
            }
            other => panic!("expected MemoryLimitExceeded error, got {other:?}"),
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    #[ignore = "requires the pinned Linux runtime and a separately downloaded 105 MB GGUF fixture"]
    fn real_smollm2_fixture_loading_cancellation_stops_server() {
        let root = std::env::var_os("DOCILER_REAL_GGUF_TEST_ROOT")
            .expect("set DOCILER_REAL_GGUF_TEST_ROOT to the isolated fixture directory");
        let root = Path::new(&root);
        let paths =
            AppPaths::new(root.join("config"), root.join("data"), root.join("cache")).unwrap();
        let fixture = AssetSpec::test_fixture(
            AssetKind::Model,
            "real-smollm2",
            "SmolLM2-135M-Instruct-Q4_K_M.gguf",
            105_454_432,
            "2e8040ceae7815abe0dcb3540b9995eaa1fa0d2ca9e797d0a635ae4433c68c2d",
        );
        verify_model(&paths, fixture, &CancellationToken::new())
            .expect("real GGUF fixture must pass SHA-256 verification");
        let runtime = crate::assets::runtime_asset_for("linux", "x86_64").unwrap();
        let installed = inspect_installed_runtime_cancellable(
            &paths,
            *runtime,
            VerificationLevel::Sha256,
            None,
        );
        assert_eq!(installed.state(), RuntimeInstallState::Verified);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = run_model_probe(
            ProbeTarget {
                executable: installed.server_path(),
                install_directory: installed.path(),
                model_path: &fixture.cache_path(&paths),
                profile: LocalProfile::Lite,
            },
            || {
                verify_model(&paths, fixture, &cancellation)?;
                if inspect_installed_runtime_cancellable(
                    &paths,
                    *runtime,
                    VerificationLevel::Sha256,
                    None,
                )
                .state()
                    != RuntimeInstallState::Verified
                {
                    return Err(ModelProbeError::RuntimeInvalid);
                }
                Ok(())
            },
            None,
            &cancellation,
            LOAD_TIMEOUT,
        );
        assert_eq!(result, Err(ModelProbeError::Cancelled));
    }
}
