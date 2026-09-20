//! Read-only hardware inventory and conservative local-model preflight checks.

use std::path::{Path, PathBuf};
use std::thread;

use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

use crate::config::LocalProfile;

const GIB: u64 = 1024 * 1024 * 1024;
const GB: u64 = 1_000_000_000;
const LITE_SUPPORTED_TOTAL: u64 = 8 * GB;
const LITE_MINIMUM_TOTAL: u64 = 6 * GB;
const LITE_WORKING_SET: u64 = 11 * GIB / 2;
const LITE_MODEL_DOWNLOAD: u64 = 3_010_000_000;
const PRO_SUPPORTED_TOTAL: u64 = 16 * GB;
const PRO_WORKING_SET: u64 = 23 * GIB / 2;
const PRO_MODEL_DOWNLOAD: u64 = 6_170_000_000;
const INSTALL_RESERVE: u64 = 2 * GIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Host,
    Cgroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceleratorCandidate {
    Metal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareInventory {
    total_memory_bytes: Option<u64>,
    available_memory_bytes: Option<u64>,
    memory_scope: MemoryScope,
    logical_cpu_count: usize,
    physical_cpu_count: Option<usize>,
    recommended_threads: usize,
    cpu_features: Vec<&'static str>,
    free_disk_bytes: Option<u64>,
    disk_mount: Option<PathBuf>,
    accelerator_candidates: Vec<AcceleratorCandidate>,
}

impl HardwareInventory {
    /// Inspect memory, CPU, and the filesystem that would hold local assets.
    ///
    /// This does not create the target path, enumerate processes, contact the
    /// network, select an accelerator, or persist the result.
    pub fn inspect(asset_path: &Path) -> Self {
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_memory(MemoryRefreshKind::nothing().with_ram())
                .with_cpu(CpuRefreshKind::everything().without_cpu_usage()),
        );
        let host_total = nonzero(system.total_memory());
        let host_available = nonzero(system.available_memory());
        let (total_memory_bytes, available_memory_bytes, memory_scope) = system
            .cgroup_limits()
            .filter(|limits| {
                limits.total_memory > 0 && host_total.is_none_or(|host| limits.total_memory < host)
            })
            .map(|limits| {
                (
                    Some(limits.total_memory),
                    nonzero(limits.free_memory),
                    MemoryScope::Cgroup,
                )
            })
            .unwrap_or((host_total, host_available, MemoryScope::Host));

        let logical_cpu_count = thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let physical_cpu_count = system.physical_core_count().filter(|count| *count > 0);
        let recommended_threads = logical_cpu_count.saturating_sub(1).max(1);
        let (disk_mount, free_disk_bytes) = disk_space_for(asset_path)
            .map(|(mount, bytes)| (Some(mount), nonzero(bytes)))
            .unwrap_or((None, None));

        Self {
            total_memory_bytes,
            available_memory_bytes,
            memory_scope,
            logical_cpu_count,
            physical_cpu_count,
            recommended_threads,
            cpu_features: detected_cpu_features(),
            free_disk_bytes,
            disk_mount,
            accelerator_candidates: accelerator_candidates(),
        }
    }

    pub fn total_memory_bytes(&self) -> Option<u64> {
        self.total_memory_bytes
    }

    pub fn available_memory_bytes(&self) -> Option<u64> {
        self.available_memory_bytes
    }

    pub fn memory_scope(&self) -> MemoryScope {
        self.memory_scope
    }

    pub fn logical_cpu_count(&self) -> usize {
        self.logical_cpu_count
    }

    pub fn physical_cpu_count(&self) -> Option<usize> {
        self.physical_cpu_count
    }

    pub fn recommended_threads(&self) -> usize {
        self.recommended_threads
    }

    pub fn cpu_features(&self) -> &[&'static str] {
        &self.cpu_features
    }

    pub fn free_disk_bytes(&self) -> Option<u64> {
        self.free_disk_bytes
    }

    pub fn disk_mount(&self) -> Option<&Path> {
        self.disk_mount.as_deref()
    }

    pub fn accelerator_candidates(&self) -> &[AcceleratorCandidate] {
        &self.accelerator_candidates
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRequirements {
    profile: LocalProfile,
    supported_total_memory_bytes: u64,
    minimum_total_memory_bytes: u64,
    required_available_memory_bytes: u64,
    approximate_model_bytes: u64,
    required_free_disk_bytes: u64,
    context_tokens: u32,
}

impl ModelRequirements {
    pub fn for_profile(profile: LocalProfile) -> Self {
        let (
            supported_total_memory_bytes,
            minimum_total_memory_bytes,
            required_available_memory_bytes,
            approximate_model_bytes,
            context_tokens,
        ) = match profile {
            LocalProfile::Lite => (
                LITE_SUPPORTED_TOTAL,
                LITE_MINIMUM_TOTAL,
                LITE_WORKING_SET,
                LITE_MODEL_DOWNLOAD,
                8_192,
            ),
            LocalProfile::Pro => (
                PRO_SUPPORTED_TOTAL,
                PRO_SUPPORTED_TOTAL,
                PRO_WORKING_SET,
                PRO_MODEL_DOWNLOAD,
                16_384,
            ),
        };
        Self {
            profile,
            supported_total_memory_bytes,
            minimum_total_memory_bytes,
            required_available_memory_bytes,
            approximate_model_bytes,
            required_free_disk_bytes: approximate_model_bytes + INSTALL_RESERVE,
            context_tokens,
        }
    }

    pub fn profile(self) -> LocalProfile {
        self.profile
    }

    pub fn supported_total_memory_bytes(self) -> u64 {
        self.supported_total_memory_bytes
    }

    pub fn minimum_total_memory_bytes(self) -> u64 {
        self.minimum_total_memory_bytes
    }

    pub fn required_available_memory_bytes(self) -> u64 {
        self.required_available_memory_bytes
    }

    pub fn approximate_model_bytes(self) -> u64 {
        self.approximate_model_bytes
    }

    pub fn required_free_disk_bytes(self) -> u64 {
        self.required_free_disk_bytes
    }

    pub fn context_tokens(self) -> u32 {
        self.context_tokens
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightStatus {
    ReadyForRuntimeProbe,
    ExperimentalForRuntimeProbe,
    InsufficientTotalMemory,
    InsufficientAvailableMemory,
    InsufficientDisk,
    InventoryIncomplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPreflight {
    requirements: ModelRequirements,
    status: PreflightStatus,
}

impl ModelPreflight {
    pub fn evaluate(profile: LocalProfile, hardware: &HardwareInventory) -> Self {
        let requirements = ModelRequirements::for_profile(profile);
        let status = match hardware.total_memory_bytes {
            Some(total) if total < requirements.minimum_total_memory_bytes => {
                PreflightStatus::InsufficientTotalMemory
            }
            None => PreflightStatus::InventoryIncomplete,
            Some(_) => match hardware.available_memory_bytes {
                Some(available) if available < requirements.required_available_memory_bytes => {
                    PreflightStatus::InsufficientAvailableMemory
                }
                None => PreflightStatus::InventoryIncomplete,
                Some(_) => match hardware.free_disk_bytes {
                    Some(free) if free < requirements.required_free_disk_bytes => {
                        PreflightStatus::InsufficientDisk
                    }
                    None => PreflightStatus::InventoryIncomplete,
                    Some(_)
                        if profile == LocalProfile::Lite
                            && hardware.total_memory_bytes
                                < Some(requirements.supported_total_memory_bytes) =>
                    {
                        PreflightStatus::ExperimentalForRuntimeProbe
                    }
                    Some(_) => PreflightStatus::ReadyForRuntimeProbe,
                },
            },
        };
        Self {
            requirements,
            status,
        }
    }

    pub fn requirements(self) -> ModelRequirements {
        self.requirements
    }

    pub fn status(self) -> PreflightStatus {
        self.status
    }
}

fn nonzero(value: u64) -> Option<u64> {
    (value > 0).then_some(value)
}

fn disk_space_for(path: &Path) -> Option<(PathBuf, u64)> {
    Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count())
        .map(|disk| (disk.mount_point().to_owned(), disk.available_space()))
}

fn accelerator_candidates() -> Vec<AcceleratorCandidate> {
    #[cfg(target_os = "macos")]
    {
        vec![AcceleratorCandidate::Metal]
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

fn detected_cpu_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("sse4.2") {
            features.push("sse4.2");
        }
        if std::is_x86_feature_detected!("avx") {
            features.push("avx");
        }
        if std::is_x86_feature_detected!("avx2") {
            features.push("avx2");
        }
        if std::is_x86_feature_detected!("fma") {
            features.push("fma");
        }
        if std::is_x86_feature_detected!("avx512f") {
            features.push("avx512f");
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            features.push("neon");
        }
    }
    features
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hardware(
        total: Option<u64>,
        available: Option<u64>,
        disk: Option<u64>,
    ) -> HardwareInventory {
        HardwareInventory {
            total_memory_bytes: total,
            available_memory_bytes: available,
            memory_scope: MemoryScope::Host,
            logical_cpu_count: 4,
            physical_cpu_count: Some(2),
            recommended_threads: 3,
            cpu_features: Vec::new(),
            free_disk_bytes: disk,
            disk_mount: None,
            accelerator_candidates: Vec::new(),
        }
    }

    #[test]
    fn below_six_gib_rejects_local_tiers() {
        let inventory = hardware(Some(5 * GIB), Some(5 * GIB), Some(20 * GIB));
        for profile in [LocalProfile::Lite, LocalProfile::Pro] {
            assert_eq!(
                ModelPreflight::evaluate(profile, &inventory).status(),
                PreflightStatus::InsufficientTotalMemory
            );
        }
    }

    #[test]
    fn lite_between_six_and_eight_gib_is_experimental_only() {
        let inventory = hardware(Some(7 * GIB), Some(6 * GIB), Some(20 * GIB));
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Lite, &inventory).status(),
            PreflightStatus::ExperimentalForRuntimeProbe
        );
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Pro, &inventory).status(),
            PreflightStatus::InsufficientTotalMemory
        );
    }

    #[test]
    fn current_memory_and_disk_fail_before_a_runtime_probe() {
        let low_memory = hardware(Some(16 * GIB), Some(5 * GIB), Some(20 * GIB));
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Lite, &low_memory).status(),
            PreflightStatus::InsufficientAvailableMemory
        );
        let low_disk = hardware(Some(16 * GIB), Some(12 * GIB), Some(4 * GIB));
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Lite, &low_disk).status(),
            PreflightStatus::InsufficientDisk
        );
    }

    #[test]
    fn supported_hosts_are_only_ready_for_a_runtime_probe() {
        let inventory = hardware(Some(16 * GIB), Some(12 * GIB), Some(20 * GIB));
        for profile in [LocalProfile::Lite, LocalProfile::Pro] {
            assert_eq!(
                ModelPreflight::evaluate(profile, &inventory).status(),
                PreflightStatus::ReadyForRuntimeProbe
            );
        }
    }

    #[test]
    fn advertised_decimal_ram_classes_are_not_treated_as_gibibytes() {
        let lite = hardware(Some(8 * GB), Some(6 * GIB), Some(20 * GIB));
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Lite, &lite).status(),
            PreflightStatus::ReadyForRuntimeProbe
        );
        let pro = hardware(Some(16 * GB), Some(12 * GIB), Some(20 * GIB));
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Pro, &pro).status(),
            PreflightStatus::ReadyForRuntimeProbe
        );
    }

    #[test]
    fn missing_inventory_fails_closed() {
        let missing_memory = hardware(None, None, Some(20 * GIB));
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Lite, &missing_memory).status(),
            PreflightStatus::InventoryIncomplete
        );
        let missing_disk = hardware(Some(8 * GIB), Some(6 * GIB), None);
        assert_eq!(
            ModelPreflight::evaluate(LocalProfile::Lite, &missing_disk).status(),
            PreflightStatus::InventoryIncomplete
        );
    }

    #[test]
    fn live_inventory_is_read_only_and_has_a_cpu_budget() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("not-created/models");
        let inventory = HardwareInventory::inspect(&target);
        assert!(!target.exists());
        assert!(inventory.logical_cpu_count() >= 1);
        assert!((1..=inventory.logical_cpu_count()).contains(&inventory.recommended_threads()));
        if sysinfo::IS_SUPPORTED_SYSTEM {
            assert!(inventory.total_memory_bytes().is_some());
            assert!(inventory.available_memory_bytes().is_some());
        }
    }
}
