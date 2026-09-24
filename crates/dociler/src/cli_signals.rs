//! Scoped signal-to-cancellation bridge for scriptable local probes.
//!
//! The probe remains responsible for stopping and reaping its child. This
//! bridge only requests cancellation; it never exits from a signal handler.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use dociler_core::chat::CancellationToken;

const SIGNAL_POLL: Duration = Duration::from_millis(25);

pub(crate) struct CliSignalGuard {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl CliSignalGuard {
    pub(crate) fn install(cancellation: CancellationToken) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (ready_send, ready_receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("dociler-cli-signals".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(listen(cancellation, worker_stop, ready_send)),
                    Err(error) => {
                        let _ = ready_send.send(Err(error));
                    }
                }
            })?;
        let mut guard = Self {
            stop,
            worker: Some(worker),
        };
        match ready_receive.recv() {
            Ok(Ok(())) => Ok(guard),
            Ok(Err(error)) => {
                guard.stop();
                Err(error)
            }
            Err(_) => {
                guard.stop();
                Err(io::Error::other("signal listener stopped during setup"))
            }
        }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CliSignalGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(unix)]
async fn listen(
    cancellation: CancellationToken,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<io::Result<()>>,
) {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::Acquire) {
        if matches!(
            tokio::time::timeout(SIGNAL_POLL, interrupt.recv()).await,
            Ok(Some(()))
        ) || matches!(
            tokio::time::timeout(SIGNAL_POLL, terminate.recv()).await,
            Ok(Some(()))
        ) {
            cancellation.cancel();
            break;
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    #[test]
    fn interrupt_and_terminate_cancel_a_subprocess() {
        if let Some(marker) = std::env::var_os("DOCILER_SIGNAL_TEST_CHILD") {
            let cancellation = CancellationToken::new();
            let _guard = CliSignalGuard::install(cancellation.clone()).unwrap();
            fs::write(marker, b"ready").unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !cancellation.is_cancelled() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(
                cancellation.is_cancelled(),
                "signal did not cancel the token"
            );
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        for signal in ["-INT", "-TERM"] {
            let marker = temp.path().join(signal.trim_start_matches('-'));
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "cli_signals::tests::interrupt_and_terminate_cancel_a_subprocess",
                    "--nocapture",
                ])
                .env("DOCILER_SIGNAL_TEST_CHILD", &marker)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !marker.exists() && Instant::now() < deadline {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            assert!(marker.exists(), "listener was not ready for {signal}");
            assert!(
                Command::new("/bin/kill")
                    .args([signal, &child.id().to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                assert!(Instant::now() < deadline, "signal child did not exit");
                thread::sleep(Duration::from_millis(5));
            };
            assert!(
                status.success(),
                "signal child failed after {signal}: {status}"
            );
        }
    }
}

#[cfg(windows)]
async fn listen(
    cancellation: CancellationToken,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<io::Result<()>>,
) {
    use tokio::signal::windows;

    let mut interrupt = match windows::ctrl_c() {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut ctrl_break = match windows::ctrl_break() {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::Acquire) {
        if matches!(
            tokio::time::timeout(SIGNAL_POLL, interrupt.recv()).await,
            Ok(Some(()))
        ) || matches!(
            tokio::time::timeout(SIGNAL_POLL, ctrl_break.recv()).await,
            Ok(Some(()))
        ) {
            cancellation.cancel();
            break;
        }
    }
}
