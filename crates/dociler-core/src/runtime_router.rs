//! Dociler-managed `llama-server` loopback router lifecycle.
//!
//! Spawns and manages an authenticated loopback `llama-server` sidecar process:
//! - Bound strictly to an OS-assigned ephemeral loopback port (`127.0.0.1:<port>`).
//! - Protected with a cryptographically generated internal API key passed via `--api-key-file`.
//! - Neither raw port nor internal API key or upstream base-model identifier is leaked publicly.
//! - Spawns in an isolated process group (`process_group(0)` on Unix) with cleared environment.
//! - Bounded diagnostic capture prevents stdout/stderr pipe blockage.
//! - Clean lifecycle management: health check, request forwarding, streaming, graceful stop/reap on drop.

use std::fmt;
use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use reqwest::redirect::Policy;
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::cancellation::CancellationToken;
use crate::config::LocalProfile;
use crate::runtime_probe::{
    DiagnosticCapture, ManagedChild, isolated_command, make_key, write_private_key,
};

const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(120);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Errors originating from the loopback runtime router.
#[derive(Debug)]
pub enum RuntimeRouterError {
    Spawn(String),
    HealthCheckFailed(String),
    ProcessExited,
    AuthenticationFailed,
    ConnectionFailed(String),
    InvalidResponse(String),
    TimedOut,
    Cancelled,
    StopFailed,
    Io(io::ErrorKind),
}

impl fmt::Display for RuntimeRouterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(err) => write!(f, "failed to spawn runtime router: {err}"),
            Self::HealthCheckFailed(msg) => write!(f, "runtime health check failed: {msg}"),
            Self::ProcessExited => write!(f, "runtime router process exited unexpectedly"),
            Self::AuthenticationFailed => write!(f, "runtime router authentication failed"),
            Self::ConnectionFailed(msg) => write!(f, "connection to runtime router failed: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid response from runtime router: {msg}"),
            Self::TimedOut => write!(f, "runtime router operation timed out"),
            Self::Cancelled => write!(f, "runtime router operation was cancelled"),
            Self::StopFailed => write!(f, "runtime router process could not be stopped cleanly"),
            Self::Io(kind) => write!(f, "runtime router I/O error: {kind:?}"),
        }
    }
}

impl std::error::Error for RuntimeRouterError {}

/// Configuration for spawning a local runtime router.
#[derive(Debug, Clone)]
pub struct RuntimeRouterConfig {
    pub profile: LocalProfile,
    pub context_tokens: u32,
    pub threads: Option<usize>,
    pub gpu_layers: u32,
    pub flash_attn: bool,
}

impl RuntimeRouterConfig {
    pub fn for_profile(profile: LocalProfile) -> Self {
        let context_tokens = match profile {
            LocalProfile::Lite => 8_192,
            LocalProfile::Pro => 16_384,
        };
        Self {
            profile,
            context_tokens,
            threads: None,
            gpu_layers: 0,
            flash_attn: false,
        }
    }
}

/// Managed loopback `llama-server` process with authenticated HTTP forwarding.
pub struct RuntimeRouter {
    port: u16,
    internal_key: Zeroizing<String>,
    _temp_dir: Option<TempDir>,
    child: Option<Mutex<ManagedChild>>,
    _diagnostics: Option<Mutex<DiagnosticCapture>>,
    client: Client,
    profile: LocalProfile,
}

impl RuntimeRouter {
    /// Spawns a managed `llama-server` process on an OS-assigned loopback port
    /// with an internal private API key and isolated process group.
    pub fn spawn(
        executable: &Path,
        install_directory: &Path,
        model_path: &Path,
        config: RuntimeRouterConfig,
        cancellation: &CancellationToken,
    ) -> Result<Self, RuntimeRouterError> {
        if cancellation.is_cancelled() {
            return Err(RuntimeRouterError::Cancelled);
        }

        let temp_dir = tempfile::Builder::new()
            .prefix("dociler-router-")
            .tempdir()
            .map_err(|e| RuntimeRouterError::Io(e.kind()))?;

        let cache_dir = temp_dir.path().join("cache");
        fs::create_dir_all(&cache_dir).map_err(|e| RuntimeRouterError::Io(e.kind()))?;

        let key = make_key().map_err(|e| RuntimeRouterError::Spawn(e.to_string()))?;
        let key_file = temp_dir.path().join("internal-api-key");
        write_private_key(&key_file, &key).map_err(|e| RuntimeRouterError::Spawn(e.to_string()))?;

        let reservation = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| RuntimeRouterError::Io(e.kind()))?;
        let port = reservation
            .local_addr()
            .map_err(|e| RuntimeRouterError::Io(e.kind()))?
            .port();
        drop(reservation);

        let threads = config
            .threads
            .unwrap_or_else(|| thread::available_parallelism().map_or(1, |c| c.get().clamp(1, 4)));

        let mut command = isolated_command(executable, install_directory, &temp_dir);
        command
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--api-key-file")
            .arg(&key_file)
            .arg("--model")
            .arg(model_path)
            .arg("--alias")
            .arg(config.profile.alias())
            .arg("--ctx-size")
            .arg(config.context_tokens.to_string())
            .arg("--parallel")
            .arg("1")
            .arg("--threads")
            .arg(threads.to_string())
            .arg("--threads-http")
            .arg("1")
            .arg("--batch-size")
            .arg("128")
            .arg("--ubatch-size")
            .arg("64")
            .arg("--n-gpu-layers")
            .arg(config.gpu_layers.to_string())
            .arg("--fit")
            .arg("off")
            .arg("--flash-attn")
            .arg(if config.flash_attn { "on" } else { "off" })
            .arg("--no-context-shift")
            .arg("--no-mmproj")
            .arg("--no-webui")
            .arg("--no-slots")
            .arg("--no-cors-credentials")
            .arg("--cors-origins")
            .arg("localhost");

        let mut child = ManagedChild::spawn(&mut command)
            .map_err(|e| RuntimeRouterError::Spawn(e.to_string()))?;
        let diagnostics = DiagnosticCapture::start(&mut child.child);

        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(HTTP_REQUEST_TIMEOUT)
            .build()
            .map_err(|e| RuntimeRouterError::ConnectionFailed(e.to_string()))?;

        let router = Self {
            port,
            internal_key: key,
            _temp_dir: Some(temp_dir),
            child: Some(Mutex::new(child)),
            _diagnostics: Some(Mutex::new(diagnostics)),
            client,
            profile: config.profile,
        };

        router.wait_until_ready(cancellation, HEALTH_CHECK_TIMEOUT)?;

        Ok(router)
    }

    /// Constructs a `RuntimeRouter` pointing to a pre-existing loopback endpoint
    /// for deterministic mock-server tests.
    pub fn from_test_endpoint(port: u16, key: Zeroizing<String>, profile: LocalProfile) -> Self {
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(HTTP_REQUEST_TIMEOUT)
            .build()
            .expect("test client build cannot fail");

        Self {
            port,
            internal_key: key,
            _temp_dir: None,
            child: None,
            _diagnostics: None,
            client,
            profile,
        }
    }

    /// Internal loopback port assigned to the process.
    pub fn internal_port(&self) -> u16 {
        self.port
    }

    /// Associated model profile alias.
    pub fn profile(&self) -> LocalProfile {
        self.profile
    }

    /// Checks the health status of the running router via `GET /health`.
    pub fn check_health(&self) -> Result<bool, RuntimeRouterError> {
        let url = format!("http://127.0.0.1:{}/health", self.port);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(self.internal_key.as_str())
            .send()
            .map_err(|e| RuntimeRouterError::ConnectionFailed(e.to_string()))?;

        if resp.status().as_u16() == 200 {
            let body: serde_json::Value = resp
                .json()
                .map_err(|e| RuntimeRouterError::InvalidResponse(e.to_string()))?;
            Ok(body["status"] == "ok")
        } else {
            Ok(false)
        }
    }

    fn wait_until_ready(
        &self,
        cancellation: &CancellationToken,
        timeout: Duration,
    ) -> Result<(), RuntimeRouterError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cancellation.is_cancelled() {
                return Err(RuntimeRouterError::Cancelled);
            }

            if let Some(child_lock) = &self.child {
                let mut guard = child_lock.lock().expect("child lock poisoned");
                if let Ok(Some(_)) = guard.child.try_wait() {
                    return Err(RuntimeRouterError::ProcessExited);
                }
            }

            if let Ok(true) = self.check_health() {
                return Ok(());
            }

            thread::sleep(POLL_INTERVAL);
        }

        Err(RuntimeRouterError::TimedOut)
    }

    /// Forwards a non-streaming chat completion request to the loopback router,
    /// normalizing the response envelope so that the model alias is strictly
    /// enforced and no internal details leak.
    pub fn forward_chat_completion(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeRouterError> {
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.internal_key.as_str())
            .json(request)
            .send()
            .map_err(|e| RuntimeRouterError::ConnectionFailed(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().unwrap_or_default();
            return Err(RuntimeRouterError::InvalidResponse(format!(
                "router returned status {status}: {err_text}"
            )));
        }

        let mut payload: serde_json::Value = resp
            .json()
            .map_err(|e| RuntimeRouterError::InvalidResponse(e.to_string()))?;

        // Normalize model field to Dociler alias
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "model".to_string(),
                serde_json::Value::String(self.profile.alias().to_string()),
            );
        }

        Ok(payload)
    }

    /// Initiates a streaming chat completion request to the loopback router.
    pub fn stream_chat_completion(
        &self,
        request: &serde_json::Value,
    ) -> Result<Response, RuntimeRouterError> {
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.internal_key.as_str())
            .json(request)
            .send()
            .map_err(|e| RuntimeRouterError::ConnectionFailed(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().unwrap_or_default();
            return Err(RuntimeRouterError::InvalidResponse(format!(
                "router returned status {status}: {err_text}"
            )));
        }

        Ok(resp)
    }

    /// Stops and reaps the child process.
    pub fn stop(&mut self) -> Result<(), RuntimeRouterError> {
        if let Some(child_lock) = self.child.take() {
            let mut guard = child_lock.lock().expect("child lock poisoned");
            guard.stop().map_err(|_| RuntimeRouterError::StopFailed)?;
        }
        Ok(())
    }
}

impl Drop for RuntimeRouter {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl fmt::Debug for RuntimeRouter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeRouter")
            .field("port", &self.port)
            .field("profile", &self.profile)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_config_and_test_endpoint() {
        let config = RuntimeRouterConfig::for_profile(LocalProfile::Lite);
        assert_eq!(config.context_tokens, 8_192);
        assert_eq!(config.profile.alias(), "dociler-lite");

        let key = Zeroizing::new("fake-token-12345".to_string());
        let router = RuntimeRouter::from_test_endpoint(18999, key, LocalProfile::Lite);
        assert_eq!(router.internal_port(), 18999);
        assert_eq!(router.profile(), LocalProfile::Lite);
    }
}
