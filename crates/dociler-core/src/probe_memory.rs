//! Process-group memory observation and enforceable ceiling abort guard.
//!
//! Linux exposes per-process RSS and high-water marks in `/proc/<pid>/status`.
//! To account for the full workload, Dociler samples concurrent memory across
//! the Dociler host process (`std::process::id()`), the runtime sidecar process,
//! and any child processes spawned by the sidecar.
//!
//! When an enforceable memory ceiling is configured, the observer aborts the
//! probe cooperatively if instantaneous combined process-group RSS exceeds the
//! ceiling, protecting constrained hosts from OOM and swap thrashing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeMemoryReport {
    /// Linux server-only peak RSS (VmHWM); unavailable on other platforms.
    pub server_peak_rss_bytes: Option<u64>,
    /// Linux combined process-group peak RSS (Dociler host + server sidecar concurrent peak).
    pub process_group_peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAbortInfo {
    pub limit_bytes: u64,
    pub observed_bytes: u64,
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use super::{MemoryAbortInfo, ProbeMemoryReport};
    use crate::cancellation::CancellationToken;

    const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);

    pub(crate) struct ProcessGroupMemorySampler {
        parent_pid: u32,
        child_pid: u32,
        server_peak: Arc<AtomicU64>,
        process_group_peak: Arc<AtomicU64>,
        aborted_limit: Arc<AtomicU64>,
        aborted_observed: Arc<AtomicU64>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    struct SamplerMetrics<'a> {
        server_peak: &'a AtomicU64,
        process_group_peak: &'a AtomicU64,
        aborted_limit: &'a AtomicU64,
        aborted_observed: &'a AtomicU64,
    }

    impl ProcessGroupMemorySampler {
        pub(crate) fn start(
            parent_pid: u32,
            child_pid: u32,
            memory_ceiling_bytes: Option<u64>,
            cancellation: CancellationToken,
        ) -> Self {
            let server_peak = Arc::new(AtomicU64::new(0));
            let process_group_peak = Arc::new(AtomicU64::new(0));
            let aborted_limit = Arc::new(AtomicU64::new(0));
            let aborted_observed = Arc::new(AtomicU64::new(0));
            let stop = Arc::new(AtomicBool::new(false));

            let initial_metrics = SamplerMetrics {
                server_peak: &server_peak,
                process_group_peak: &process_group_peak,
                aborted_limit: &aborted_limit,
                aborted_observed: &aborted_observed,
            };
            sample(
                parent_pid,
                child_pid,
                &initial_metrics,
                memory_ceiling_bytes,
                &cancellation,
            );

            let worker_server = Arc::clone(&server_peak);
            let worker_pg = Arc::clone(&process_group_peak);
            let worker_limit = Arc::clone(&aborted_limit);
            let worker_observed = Arc::clone(&aborted_observed);
            let worker_stop = Arc::clone(&stop);
            let worker_cancellation = cancellation;

            let worker = thread::Builder::new()
                .name("dociler-probe-memory".to_owned())
                .spawn(move || {
                    let metrics = SamplerMetrics {
                        server_peak: &worker_server,
                        process_group_peak: &worker_pg,
                        aborted_limit: &worker_limit,
                        aborted_observed: &worker_observed,
                    };
                    while !worker_stop.load(Ordering::Acquire) {
                        sample(
                            parent_pid,
                            child_pid,
                            &metrics,
                            memory_ceiling_bytes,
                            &worker_cancellation,
                        );
                        thread::sleep(SAMPLE_INTERVAL);
                    }
                })
                .ok();

            Self {
                parent_pid,
                child_pid,
                server_peak,
                process_group_peak,
                aborted_limit,
                aborted_observed,
                stop,
                worker,
            }
        }

        pub(crate) fn abort_info(&self) -> Option<MemoryAbortInfo> {
            let limit = self.aborted_limit.load(Ordering::Acquire);
            if limit > 0 {
                let observed = self.aborted_observed.load(Ordering::Acquire);
                Some(MemoryAbortInfo {
                    limit_bytes: limit,
                    observed_bytes: observed,
                })
            } else {
                None
            }
        }

        pub(crate) fn finish(mut self) -> ProbeMemoryReport {
            let cancellation = CancellationToken::new();
            let metrics = SamplerMetrics {
                server_peak: &self.server_peak,
                process_group_peak: &self.process_group_peak,
                aborted_limit: &self.aborted_limit,
                aborted_observed: &self.aborted_observed,
            };
            sample(
                self.parent_pid,
                self.child_pid,
                &metrics,
                None,
                &cancellation,
            );
            self.stop();
            let server = self.server_peak.load(Ordering::Acquire);
            let pg = self.process_group_peak.load(Ordering::Acquire);
            ProbeMemoryReport {
                server_peak_rss_bytes: (server > 0).then_some(server),
                process_group_peak_rss_bytes: (pg > 0).then_some(pg),
            }
        }

        fn stop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    impl Drop for ProcessGroupMemorySampler {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn sample(
        parent_pid: u32,
        child_pid: u32,
        metrics: &SamplerMetrics<'_>,
        memory_ceiling_bytes: Option<u64>,
        cancellation: &CancellationToken,
    ) {
        let parent_rss = read_rss(parent_pid).unwrap_or(0);
        let mut server_rss = 0u64;
        let mut server_hwm = 0u64;

        if let Ok(status) = fs::read_to_string(format!("/proc/{child_pid}/status")) {
            if let Some(rss) = parse_labeled_bytes(&status, "VmRSS:") {
                server_rss = server_rss.saturating_add(rss);
            }
            if let Some(hwm) = parse_labeled_bytes(&status, "VmHWM:") {
                server_hwm = server_hwm.max(hwm);
            }
        }

        for descendant in child_pids(child_pid) {
            if let Ok(status) = fs::read_to_string(format!("/proc/{descendant}/status")) {
                if let Some(rss) = parse_labeled_bytes(&status, "VmRSS:") {
                    server_rss = server_rss.saturating_add(rss);
                }
                if let Some(hwm) = parse_labeled_bytes(&status, "VmHWM:") {
                    server_hwm = server_hwm.saturating_add(hwm);
                }
            }
        }

        if server_rss > 0 || server_hwm > 0 {
            let effective_server_peak = server_hwm.max(server_rss);
            metrics
                .server_peak
                .fetch_max(effective_server_peak, Ordering::AcqRel);

            let combined_concurrent = parent_rss.saturating_add(server_rss);
            metrics
                .process_group_peak
                .fetch_max(combined_concurrent, Ordering::AcqRel);
            metrics.process_group_peak.fetch_max(
                parent_rss.saturating_add(effective_server_peak),
                Ordering::AcqRel,
            );

            if let Some(ceiling) = memory_ceiling_bytes {
                let max_observed =
                    combined_concurrent.max(parent_rss.saturating_add(effective_server_peak));
                if max_observed > ceiling && metrics.aborted_limit.load(Ordering::Acquire) == 0 {
                    metrics.aborted_limit.store(ceiling, Ordering::Release);
                    metrics
                        .aborted_observed
                        .store(max_observed, Ordering::Release);
                    cancellation.cancel();
                }
            }
        }
    }

    fn read_rss(pid: u32) -> Option<u64> {
        let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        parse_labeled_bytes(&status, "VmRSS:")
    }

    fn child_pids(parent_pid: u32) -> Vec<u32> {
        if let Ok(children) =
            fs::read_to_string(format!("/proc/{parent_pid}/task/{parent_pid}/children"))
        {
            children
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .collect()
        } else {
            Vec::new()
        }
    }

    pub(crate) fn parse_labeled_bytes(status: &str, label: &str) -> Option<u64> {
        status.lines().find_map(|line| {
            let value = line.strip_prefix(label)?;
            let mut fields = value.split_whitespace();
            let kib = fields.next()?.parse::<u64>().ok()?;
            (fields.next()? == "kB" && fields.next().is_none()).then(|| kib.saturating_mul(1024))
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_only_labeled_memory_units() {
            let sample = "Name:\ttest\nVmRSS:\t2048 kB\nVmHWM:\t1234 kB\n";
            assert_eq!(parse_labeled_bytes(sample, "VmRSS:"), Some(2_097_152));
            assert_eq!(parse_labeled_bytes(sample, "VmHWM:"), Some(1_263_616));
            assert_eq!(parse_labeled_bytes("VmHWM:\t1234 MB\n", "VmHWM:"), None);
            assert_eq!(parse_labeled_bytes("VmHWM:\tbroken kB\n", "VmHWM:"), None);
            assert_eq!(parse_labeled_bytes("VmPeak:\t1234 kB\n", "VmHWM:"), None);
            assert_eq!(parse_labeled_bytes("VmRSS:\t2048 kB\n", "VmHWM:"), None);
        }

        #[test]
        fn observes_current_process_and_combined_memory() {
            let token = CancellationToken::new();
            let pid = std::process::id();
            let sampler = ProcessGroupMemorySampler::start(pid, pid, None, token);
            let report = sampler.finish();
            assert!(report.server_peak_rss_bytes.is_some_and(|bytes| bytes > 0));
            assert!(
                report
                    .process_group_peak_rss_bytes
                    .is_some_and(|bytes| bytes > 0)
            );
        }

        #[test]
        fn memory_ceiling_breach_triggers_cancellation_and_abort() {
            let token = CancellationToken::new();
            let pid = std::process::id();
            let sampler = ProcessGroupMemorySampler::start(pid, pid, Some(1), token.clone());
            assert!(token.is_cancelled());
            let abort = sampler.abort_info().expect("must record abort info");
            assert_eq!(abort.limit_bytes, 1);
            assert!(abort.observed_bytes > 1);
            let report = sampler.finish();
            assert!(
                report
                    .process_group_peak_rss_bytes
                    .is_some_and(|bytes| bytes >= abort.observed_bytes)
            );
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::ProcessGroupMemorySampler;

#[cfg(not(target_os = "linux"))]
pub(crate) struct ProcessGroupMemorySampler;

#[cfg(not(target_os = "linux"))]
impl ProcessGroupMemorySampler {
    pub(crate) fn start(
        _parent_pid: u32,
        _child_pid: u32,
        _memory_ceiling_bytes: Option<u64>,
        _cancellation: crate::cancellation::CancellationToken,
    ) -> Self {
        Self
    }

    pub(crate) fn abort_info(&self) -> Option<MemoryAbortInfo> {
        None
    }

    pub(crate) fn finish(self) -> ProbeMemoryReport {
        ProbeMemoryReport {
            server_peak_rss_bytes: None,
            process_group_peak_rss_bytes: None,
        }
    }
}
