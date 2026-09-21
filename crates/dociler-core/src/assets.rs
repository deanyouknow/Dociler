//! Immutable local-model/runtime metadata and read-only cache verification.

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::LocalProfile;
use crate::paths::AppPaths;

pub const MANIFEST_ID: &str = "dociler-assets-v1";
pub const LLAMA_CPP_RELEASE: &str = "v0.4.0";
pub const LLAMA_CPP_BUILD: &str = "b10809";
pub const LLAMA_CPP_COMMIT: &str = "5266f24da75dc449bd56cbed7addb9c8e4a6a73e";

const CACHE_VERSION_DIRECTORY: &str = "manifest-v1";
const READ_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Model,
    RuntimeArchive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetSpec {
    id: &'static str,
    kind: AssetKind,
    cache_key: &'static str,
    file_name: &'static str,
    source_url: &'static str,
    byte_size: u64,
    sha256: &'static str,
}

impl AssetSpec {
    #[cfg(test)]
    pub(crate) const fn test_fixture(
        kind: AssetKind,
        cache_key: &'static str,
        file_name: &'static str,
        byte_size: u64,
        sha256: &'static str,
    ) -> Self {
        Self {
            id: "test-fixture",
            kind,
            cache_key,
            file_name,
            source_url: "http://127.0.0.1/fixture",
            byte_size,
            sha256,
        }
    }

    pub const fn id(self) -> &'static str {
        self.id
    }

    pub const fn kind(self) -> AssetKind {
        self.kind
    }

    pub const fn file_name(self) -> &'static str {
        self.file_name
    }

    pub const fn source_url(self) -> &'static str {
        self.source_url
    }

    pub const fn byte_size(self) -> u64 {
        self.byte_size
    }

    pub const fn sha256(self) -> &'static str {
        self.sha256
    }

    pub fn cache_path(self, paths: &AppPaths) -> PathBuf {
        cache_root(paths, self.kind)
            .join(CACHE_VERSION_DIRECTORY)
            .join(self.cache_key)
            .join(self.file_name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelAsset {
    profile: LocalProfile,
    base_model: &'static str,
    repository: &'static str,
    revision: &'static str,
    license: &'static str,
    artifact: AssetSpec,
}

impl ModelAsset {
    pub const fn profile(self) -> LocalProfile {
        self.profile
    }

    pub const fn base_model(self) -> &'static str {
        self.base_model
    }

    pub const fn repository(self) -> &'static str {
        self.repository
    }

    pub const fn revision(self) -> &'static str {
        self.revision
    }

    pub const fn license(self) -> &'static str {
        self.license
    }

    pub const fn artifact(self) -> AssetSpec {
        self.artifact
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeAsset {
    operating_system: &'static str,
    architecture: &'static str,
    backend: &'static str,
    artifact: AssetSpec,
}

#[derive(Debug, Clone, Copy)]
struct RuntimeTarget {
    operating_system: &'static str,
    architecture: &'static str,
    backend: &'static str,
    cache_key: &'static str,
}

impl RuntimeAsset {
    #[cfg(test)]
    pub(crate) const fn test_fixture(
        operating_system: &'static str,
        architecture: &'static str,
        artifact: AssetSpec,
    ) -> Self {
        Self {
            operating_system,
            architecture,
            backend: "test",
            artifact,
        }
    }

    pub const fn operating_system(self) -> &'static str {
        self.operating_system
    }

    pub const fn architecture(self) -> &'static str {
        self.architecture
    }

    pub const fn backend(self) -> &'static str {
        self.backend
    }

    pub const fn artifact(self) -> AssetSpec {
        self.artifact
    }
}

pub const MODEL_ASSETS: [ModelAsset; 2] = [
    ModelAsset {
        profile: LocalProfile::Lite,
        base_model: "Qwen3.5 4B",
        repository: "bartowski/Qwen_Qwen3.5-4B-GGUF",
        revision: "ba06320255db2dbec194dad738d066be90dabf29",
        license: "Apache-2.0",
        artifact: AssetSpec {
            id: "qwen3.5-4b-q4-k-m",
            kind: AssetKind::Model,
            cache_key: "dociler-lite",
            file_name: "Qwen_Qwen3.5-4B-Q4_K_M.gguf",
            source_url: "https://huggingface.co/bartowski/Qwen_Qwen3.5-4B-GGUF/resolve/ba06320255db2dbec194dad738d066be90dabf29/Qwen_Qwen3.5-4B-Q4_K_M.gguf?download=true",
            byte_size: 3_013_027_808,
            sha256: "13c16f426047e2de38cd075bdade4a7bcbc8c774384876f677740cda65f8a983",
        },
    },
    ModelAsset {
        profile: LocalProfile::Pro,
        base_model: "Qwen3.5 9B",
        repository: "bartowski/Qwen_Qwen3.5-9B-GGUF",
        revision: "2dcd842c59ea5eb119267064550a7a4c592b16c3",
        license: "Apache-2.0",
        artifact: AssetSpec {
            id: "qwen3.5-9b-q4-k-m",
            kind: AssetKind::Model,
            cache_key: "dociler-pro",
            file_name: "Qwen_Qwen3.5-9B-Q4_K_M.gguf",
            source_url: "https://huggingface.co/bartowski/Qwen_Qwen3.5-9B-GGUF/resolve/2dcd842c59ea5eb119267064550a7a4c592b16c3/Qwen_Qwen3.5-9B-Q4_K_M.gguf?download=true",
            byte_size: 6_169_341_984,
            sha256: "d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43",
        },
    },
];

pub const RUNTIME_ASSETS: [RuntimeAsset; 5] = [
    runtime_asset(
        RuntimeTarget {
            operating_system: "macos",
            architecture: "aarch64",
            backend: "metal",
            cache_key: "macos-aarch64-metal",
        },
        "llama-b10809-bin-macos-arm64.tar.gz",
        "https://github.com/ggml-org/llama.cpp/releases/download/b10809/llama-b10809-bin-macos-arm64.tar.gz",
        11_123_196,
        "7d692df9e1e386e62f1c12b843903218041e6cd74c9415aa39a7ed3176f9eaa2",
    ),
    runtime_asset(
        RuntimeTarget {
            operating_system: "macos",
            architecture: "x86_64",
            backend: "metal",
            cache_key: "macos-x86_64-metal",
        },
        "llama-b10809-bin-macos-x64.tar.gz",
        "https://github.com/ggml-org/llama.cpp/releases/download/b10809/llama-b10809-bin-macos-x64.tar.gz",
        11_175_330,
        "13b34aa8a5d87341a21065a83f54a8167e1aaa6fe0d66065de01632a1ed64be6",
    ),
    runtime_asset(
        RuntimeTarget {
            operating_system: "linux",
            architecture: "aarch64",
            backend: "cpu",
            cache_key: "linux-aarch64-cpu",
        },
        "llama-b10809-bin-ubuntu-arm64.tar.gz",
        "https://github.com/ggml-org/llama.cpp/releases/download/b10809/llama-b10809-bin-ubuntu-arm64.tar.gz",
        13_380_118,
        "f2b7333971e1b7b42e9268bfdbfa30f5f56e2897156084d2251385df94aec358",
    ),
    runtime_asset(
        RuntimeTarget {
            operating_system: "linux",
            architecture: "x86_64",
            backend: "cpu",
            cache_key: "linux-x86_64-cpu",
        },
        "llama-b10809-bin-ubuntu-x64.tar.gz",
        "https://github.com/ggml-org/llama.cpp/releases/download/b10809/llama-b10809-bin-ubuntu-x64.tar.gz",
        16_734_586,
        "5e34434ddc6d03cd1584f403201aff0d4bd1a5793a72ff7e286532dfd1e4b941",
    ),
    runtime_asset(
        RuntimeTarget {
            operating_system: "windows",
            architecture: "x86_64",
            backend: "cpu",
            cache_key: "windows-x86_64-cpu",
        },
        "llama-b10809-bin-win-cpu-x64.zip",
        "https://github.com/ggml-org/llama.cpp/releases/download/b10809/llama-b10809-bin-win-cpu-x64.zip",
        18_407_457,
        "9df3158ed228a641a4b127942d7f459f24c9e13f04682659d05c00c80099b6b5",
    ),
];

const fn runtime_asset(
    target: RuntimeTarget,
    file_name: &'static str,
    source_url: &'static str,
    byte_size: u64,
    sha256: &'static str,
) -> RuntimeAsset {
    RuntimeAsset {
        operating_system: target.operating_system,
        architecture: target.architecture,
        backend: target.backend,
        artifact: AssetSpec {
            id: file_name,
            kind: AssetKind::RuntimeArchive,
            cache_key: target.cache_key,
            file_name,
            source_url,
            byte_size,
            sha256,
        },
    }
}

pub fn model_asset(profile: LocalProfile) -> &'static ModelAsset {
    match profile {
        LocalProfile::Lite => &MODEL_ASSETS[0],
        LocalProfile::Pro => &MODEL_ASSETS[1],
    }
}

pub fn runtime_asset_for(
    operating_system: &str,
    architecture: &str,
) -> Option<&'static RuntimeAsset> {
    RUNTIME_ASSETS.iter().find(|asset| {
        asset.operating_system == operating_system && asset.architecture == architecture
    })
}

pub fn current_runtime_asset() -> Option<&'static RuntimeAsset> {
    runtime_asset_for(std::env::consts::OS, std::env::consts::ARCH)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationLevel {
    MetadataOnly,
    Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheState {
    Missing,
    PresentUnverified,
    Verified,
    SizeMismatch { expected: u64, observed: u64 },
    HashMismatch { observed: String },
    UnsafeFileType,
    Unreadable(io::ErrorKind),
}

impl CacheState {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheInspection {
    path: PathBuf,
    state: CacheState,
}

impl CacheInspection {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn state(&self) -> &CacheState {
        &self.state
    }
}

pub fn inspect_cached_asset(
    paths: &AppPaths,
    asset: AssetSpec,
    level: VerificationLevel,
) -> CacheInspection {
    let root = cache_root(paths, asset.kind);
    let path = asset.cache_path(paths);
    let state = inspect_path(&root, &path, asset, level);
    CacheInspection { path, state }
}

fn cache_root(paths: &AppPaths, kind: AssetKind) -> PathBuf {
    match kind {
        AssetKind::Model => paths.models_dir(),
        AssetKind::RuntimeArchive => paths.runtimes_dir(),
    }
}

fn inspect_path(
    root: &Path,
    path: &Path,
    asset: AssetSpec,
    level: VerificationLevel,
) -> CacheState {
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return CacheState::Missing,
        Err(error) => return CacheState::Unreadable(error.kind()),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return CacheState::UnsafeFileType;
    }
    let Some(relative) = path.strip_prefix(root).ok() else {
        return CacheState::UnsafeFileType;
    };
    let mut current = root.to_path_buf();
    let component_count = relative.components().count();

    for (index, component) in relative.components().enumerate() {
        current.push(component);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return CacheState::Missing,
            Err(error) => return CacheState::Unreadable(error.kind()),
        };
        let is_final = index + 1 == component_count;
        if metadata.file_type().is_symlink()
            || (is_final && !metadata.file_type().is_file())
            || (!is_final && !metadata.file_type().is_dir())
        {
            return CacheState::UnsafeFileType;
        }
        if is_final && metadata.len() != asset.byte_size {
            return CacheState::SizeMismatch {
                expected: asset.byte_size,
                observed: metadata.len(),
            };
        }
    }

    if component_count == 0 {
        return CacheState::UnsafeFileType;
    }
    if level == VerificationLevel::MetadataOnly {
        return CacheState::PresentUnverified;
    }

    verify_file(path, asset)
}

fn verify_file(path: &Path, asset: AssetSpec) -> CacheState {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return CacheState::Unreadable(error.kind()),
    };
    let mut reader =
        BufReader::with_capacity(READ_BUFFER_BYTES, file).take(asset.byte_size.saturating_add(1));
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    let mut observed = 0_u64;

    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => return CacheState::Unreadable(error.kind()),
        };
        observed = observed.saturating_add(count as u64);
        hasher.update(&buffer[..count]);
    }

    if observed != asset.byte_size {
        return CacheState::SizeMismatch {
            expected: asset.byte_size,
            observed,
        };
    }
    let observed_hash = format!("{:x}", hasher.finalize());
    if observed_hash == asset.sha256 {
        CacheState::Verified
    } else {
        CacheState::HashMismatch {
            observed: observed_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO_HASH: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn paths(root: &Path) -> AppPaths {
        AppPaths::new(root.join("config"), root.join("data"), root.join("cache")).unwrap()
    }

    fn fixture_asset() -> AssetSpec {
        AssetSpec {
            id: "fixture",
            kind: AssetKind::Model,
            cache_key: "fixture",
            file_name: "fixture.gguf",
            source_url: "https://example.invalid/fixture.gguf",
            byte_size: 5,
            sha256: HELLO_HASH,
        }
    }

    #[test]
    fn manifest_has_unique_safe_immutable_entries() {
        let mut ids = std::collections::BTreeSet::new();
        let mut paths = std::collections::BTreeSet::new();
        for artifact in MODEL_ASSETS
            .iter()
            .map(|model| model.artifact)
            .chain(RUNTIME_ASSETS.iter().map(|runtime| runtime.artifact))
        {
            assert!(ids.insert(artifact.id));
            assert!(paths.insert((artifact.kind as u8, artifact.cache_key, artifact.file_name)));
            assert!(artifact.source_url.starts_with("https://"));
            assert!(!artifact.source_url.contains("/main/"));
            assert!(artifact.byte_size > 0);
            assert_eq!(artifact.sha256.len(), 64);
            assert!(artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert_eq!(Path::new(artifact.cache_key).components().count(), 1);
            assert_eq!(Path::new(artifact.file_name).components().count(), 1);
        }
        assert_eq!(ids.len(), 7);
    }

    #[test]
    fn platform_runtime_mapping_is_explicit() {
        assert_eq!(
            runtime_asset_for("linux", "x86_64").unwrap().backend(),
            "cpu"
        );
        assert_eq!(
            runtime_asset_for("macos", "aarch64").unwrap().backend(),
            "metal"
        );
        assert!(runtime_asset_for("windows", "aarch64").is_none());
        assert!(runtime_asset_for("freebsd", "x86_64").is_none());
    }

    #[test]
    fn missing_inspection_creates_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let paths = paths(directory.path());
        let inspection =
            inspect_cached_asset(&paths, fixture_asset(), VerificationLevel::MetadataOnly);
        assert_eq!(inspection.state(), &CacheState::Missing);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn metadata_and_hash_verification_are_separate() {
        let directory = tempfile::tempdir().unwrap();
        let paths = paths(directory.path());
        let artifact = fixture_asset();
        let path = artifact.cache_path(&paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"hello").unwrap();

        assert_eq!(
            inspect_cached_asset(&paths, artifact, VerificationLevel::MetadataOnly).state(),
            &CacheState::PresentUnverified
        );
        assert_eq!(
            inspect_cached_asset(&paths, artifact, VerificationLevel::Sha256).state(),
            &CacheState::Verified
        );
    }

    #[test]
    fn size_and_hash_mismatches_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let paths = paths(directory.path());
        let artifact = fixture_asset();
        let path = artifact.cache_path(&paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        fs::write(&path, b"too long").unwrap();
        assert!(matches!(
            inspect_cached_asset(&paths, artifact, VerificationLevel::Sha256).state(),
            CacheState::SizeMismatch { .. }
        ));

        fs::write(&path, b"HELLO").unwrap();
        assert!(matches!(
            inspect_cached_asset(&paths, artifact, VerificationLevel::Sha256).state(),
            CacheState::HashMismatch { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_non_regular_files_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let paths = paths(directory.path());
        let artifact = fixture_asset();
        let path = artifact.cache_path(&paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let outside = directory.path().join("outside");
        fs::write(&outside, b"hello").unwrap();
        symlink(&outside, &path).unwrap();
        assert_eq!(
            inspect_cached_asset(&paths, artifact, VerificationLevel::Sha256).state(),
            &CacheState::UnsafeFileType
        );
    }
}
