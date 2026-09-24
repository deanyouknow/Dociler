//! Diagnostic child memory observation, not a release memory admission gate.
//!
//! Linux exposes a per-process high-water RSS in `/proc/<pid>/status`. Sampling
//! it while the child is alive avoids relying on only the final instant. This
//! excludes the Dociler parent, any descendant processes, parser/index work,
//! and GPU memory, so callers must not label it process-group qualification.

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);

    pub(crate) struct ChildMemorySampler {
        pid: u32,
        peak: Arc<AtomicU64>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl ChildMemorySampler {
        pub(crate) fn start(pid: u32) -> Self {
            let peak = Arc::new(AtomicU64::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            sample(pid, &peak);
            let worker_peak = Arc::clone(&peak);
            let worker_stop = Arc::clone(&stop);
            let worker = thread::Builder::new()
                .name("dociler-probe-memory".to_owned())
                .spawn(move || {
                    while !worker_stop.load(Ordering::Acquire) {
                        sample(pid, &worker_peak);
                        thread::sleep(SAMPLE_INTERVAL);
                    }
                })
                .ok();
            Self {
                pid,
                peak,
                stop,
                worker,
            }
        }

        pub(crate) fn finish(mut self) -> Option<u64> {
            sample(self.pid, &self.peak);
            self.stop();
            let peak = self.peak.load(Ordering::Acquire);
            (peak > 0).then_some(peak)
        }

        fn stop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    impl Drop for ChildMemorySampler {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn sample(pid: u32, peak: &AtomicU64) {
        if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) {
            if let Some(bytes) = parse_high_water_bytes(&status) {
                peak.fetch_max(bytes, Ordering::AcqRel);
            }
        }
    }

    fn parse_high_water_bytes(status: &str) -> Option<u64> {
        status.lines().find_map(|line| {
            let value = line.strip_prefix("VmHWM:")?;
            let mut fields = value.split_whitespace();
            let kib = fields.next()?.parse::<u64>().ok()?;
            (fields.next()? == "kB" && fields.next().is_none()).then(|| kib.saturating_mul(1024))
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_only_labeled_high_water_rss() {
            assert_eq!(
                parse_high_water_bytes("Name:\ttest\nVmRSS:\t2048 kB\nVmHWM:\t1234 kB\n"),
                Some(1_263_616)
            );
            assert_eq!(parse_high_water_bytes("VmHWM:\t1234 MB\n"), None);
            assert_eq!(parse_high_water_bytes("VmHWM:\tbroken kB\n"), None);
            assert_eq!(parse_high_water_bytes("VmRSS:\t1234 kB\n"), None);
        }

        #[test]
        fn observes_current_process_without_persistence() {
            let sampler = ChildMemorySampler::start(std::process::id());
            assert!(sampler.finish().is_some_and(|value| value > 0));
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::ChildMemorySampler;

#[cfg(not(target_os = "linux"))]
pub(crate) struct ChildMemorySampler;

#[cfg(not(target_os = "linux"))]
impl ChildMemorySampler {
    pub(crate) fn start(_pid: u32) -> Self {
        Self
    }

    pub(crate) fn finish(self) -> Option<u64> {
        None
    }
}
