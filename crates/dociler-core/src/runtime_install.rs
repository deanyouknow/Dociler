//! Safe, bounded installation of a verified pinned llama.cpp runtime archive.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::assets::{
    AssetSpec, CacheState, LLAMA_CPP_BUILD, MANIFEST_ID, RuntimeAsset, VerificationLevel,
    inspect_cached_asset_cancellable,
};
use crate::cancellation::CancellationToken;
use crate::paths::AppPaths;

const INSTALL_DIRECTORY: &str = "runtime";
const INVENTORY_FILE: &str = ".dociler-runtime.json";
const INSTALL_LOCK: &str = ".dociler-install.lock";
const MAX_ARCHIVE_ENTRIES: usize = 256;
const MAX_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 512;
const MAX_INVENTORY_BYTES: u64 = 256 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeInstallOptions {
    pub confirmed: bool,
}

impl RuntimeInstallOptions {
    pub const fn confirmed() -> Self {
        Self { confirmed: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeInstallOutcome {
    AlreadyInstalled,
    Installed {
        file_count: usize,
        expanded_bytes: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeInstallState {
    Missing,
    PresentUnverified,
    Verified,
    Invalid,
    Cancelled,
}

impl RuntimeInstallState {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified)
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInstallInspection {
    path: PathBuf,
    server_path: PathBuf,
    state: RuntimeInstallState,
}

impl RuntimeInstallInspection {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn server_path(&self) -> &Path {
        &self.server_path
    }

    pub fn state(&self) -> RuntimeInstallState {
        self.state
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeInstallError {
    ConsentRequired,
    ArchiveMissing,
    ArchiveUnverified,
    UnsupportedArchive,
    UnsafeArchive,
    LimitsExceeded,
    ExpectedServerMissing,
    ExistingInvalid,
    Busy,
    Cancelled,
    PublishConflict,
    Io(io::ErrorKind),
}

impl fmt::Display for RuntimeInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ConsentRequired => "runtime installation consent is required",
            Self::ArchiveMissing => "the pinned runtime archive is not cached",
            Self::ArchiveUnverified => "the cached runtime archive is not verified",
            Self::UnsupportedArchive => "the runtime archive format is unsupported",
            Self::UnsafeArchive => "the runtime archive contains an unsafe entry",
            Self::LimitsExceeded => "the runtime archive exceeds extraction limits",
            Self::ExpectedServerMissing => "the expected llama-server executable is missing",
            Self::ExistingInvalid => "an invalid installed runtime already exists",
            Self::Busy => "another process is installing this runtime",
            Self::Cancelled => "runtime installation was cancelled",
            Self::PublishConflict => "the runtime install path changed while publishing",
            Self::Io(_) => "runtime installation I/O failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RuntimeInstallError {}

#[derive(Debug, Clone, Copy)]
enum ArchiveFormat {
    TarGz,
    Zip,
}

#[derive(Debug, Clone)]
struct ArchiveLayout {
    format: ArchiveFormat,
    root: Option<String>,
    server_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeInventory {
    schema: u32,
    manifest_id: String,
    asset_id: String,
    archive_sha256: String,
    server_file: String,
    files: Vec<InventoryFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug)]
struct PendingLink {
    path: PathBuf,
    target: PathBuf,
}

/// Install a verified cached runtime archive into a private versioned directory.
/// This function never launches or probes the resulting executable.
pub fn install_cached_runtime(
    paths: &AppPaths,
    runtime: RuntimeAsset,
    options: RuntimeInstallOptions,
    cancellation: &CancellationToken,
) -> Result<RuntimeInstallOutcome, RuntimeInstallError> {
    if !options.confirmed {
        return Err(RuntimeInstallError::ConsentRequired);
    }
    if cancellation.is_cancelled() {
        return Err(RuntimeInstallError::Cancelled);
    }
    let asset = runtime.artifact();
    match inspect_cached_asset_cancellable(
        paths,
        asset,
        VerificationLevel::Sha256,
        Some(cancellation),
    )
    .state()
    {
        CacheState::Verified => {}
        CacheState::Cancelled => return Err(RuntimeInstallError::Cancelled),
        CacheState::Missing => return Err(RuntimeInstallError::ArchiveMissing),
        _ => return Err(RuntimeInstallError::ArchiveUnverified),
    }
    let layout = archive_layout(runtime)?;
    let archive_path = asset.cache_path(paths);
    let parent = archive_path
        .parent()
        .ok_or(RuntimeInstallError::UnsafeArchive)?;
    let lock = open_private_file(&parent.join(INSTALL_LOCK), true, false)?;
    FileExt::try_lock_exclusive(&lock).map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            RuntimeInstallError::Busy
        } else {
            RuntimeInstallError::Io(error.kind())
        }
    })?;

    match inspect_installed_runtime_cancellable(
        paths,
        runtime,
        VerificationLevel::Sha256,
        Some(cancellation),
    )
    .state()
    {
        RuntimeInstallState::Verified => return Ok(RuntimeInstallOutcome::AlreadyInstalled),
        RuntimeInstallState::Cancelled => return Err(RuntimeInstallError::Cancelled),
        RuntimeInstallState::Missing => {}
        RuntimeInstallState::PresentUnverified => {
            return Ok(RuntimeInstallOutcome::AlreadyInstalled);
        }
        RuntimeInstallState::Invalid => return Err(RuntimeInstallError::ExistingInvalid),
    }

    let archive = open_verified_archive(&archive_path, asset, cancellation)?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(".runtime-staging-");
    let staging = builder
        .tempdir_in(parent)
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    set_private_directory(staging.path())?;
    let mut files = extract_archive(archive, staging.path(), &layout, cancellation)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let server_present = files.iter().any(|file| file.path == layout.server_file);
    if !server_present {
        return Err(RuntimeInstallError::ExpectedServerMissing);
    }
    let expanded_bytes = files.iter().map(|file| file.bytes).sum();
    let inventory = RuntimeInventory {
        schema: 1,
        manifest_id: MANIFEST_ID.to_owned(),
        asset_id: asset.id().to_owned(),
        archive_sha256: asset.sha256().to_owned(),
        server_file: layout.server_file.clone(),
        files,
    };
    write_inventory(staging.path(), &inventory)?;
    sync_directory(staging.path())?;
    if verify_install_at(
        staging.path(),
        runtime,
        VerificationLevel::Sha256,
        Some(cancellation),
    ) != RuntimeInstallState::Verified
    {
        if cancellation.is_cancelled() {
            return Err(RuntimeInstallError::Cancelled);
        }
        return Err(RuntimeInstallError::UnsafeArchive);
    }

    let final_path = install_path(paths, runtime);
    match fs::rename(staging.path(), &final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return if inspect_installed_runtime_cancellable(
                paths,
                runtime,
                VerificationLevel::Sha256,
                Some(cancellation),
            )
            .state()
                == RuntimeInstallState::Verified
            {
                Ok(RuntimeInstallOutcome::AlreadyInstalled)
            } else {
                Err(RuntimeInstallError::PublishConflict)
            };
        }
        Err(error) => return Err(RuntimeInstallError::Io(error.kind())),
    }
    sync_directory(parent)?;
    Ok(RuntimeInstallOutcome::Installed {
        file_count: inventory.files.len(),
        expanded_bytes,
    })
}

pub fn inspect_installed_runtime(
    paths: &AppPaths,
    runtime: RuntimeAsset,
    level: VerificationLevel,
) -> RuntimeInstallInspection {
    inspect_installed_runtime_cancellable(paths, runtime, level, None)
}

pub fn inspect_installed_runtime_cancellable(
    paths: &AppPaths,
    runtime: RuntimeAsset,
    level: VerificationLevel,
    cancellation: Option<&CancellationToken>,
) -> RuntimeInstallInspection {
    let path = install_path(paths, runtime);
    let server_path = path.join(expected_server_file(runtime));
    let state = verify_install_at(&path, runtime, level, cancellation);
    RuntimeInstallInspection {
        path,
        server_path,
        state,
    }
}

fn install_path(paths: &AppPaths, runtime: RuntimeAsset) -> PathBuf {
    runtime
        .artifact()
        .cache_path(paths)
        .parent()
        .expect("manifest cache paths always have a parent")
        .join(INSTALL_DIRECTORY)
}

fn expected_server_file(runtime: RuntimeAsset) -> &'static str {
    if runtime.operating_system() == "windows" {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn archive_layout(runtime: RuntimeAsset) -> Result<ArchiveLayout, RuntimeInstallError> {
    let file_name = runtime.artifact().file_name();
    if file_name.ends_with(".tar.gz") && runtime.operating_system() != "windows" {
        Ok(ArchiveLayout {
            format: ArchiveFormat::TarGz,
            root: Some(format!("llama-{LLAMA_CPP_BUILD}")),
            server_file: expected_server_file(runtime).to_owned(),
        })
    } else if file_name.ends_with(".zip") && runtime.operating_system() == "windows" {
        Ok(ArchiveLayout {
            format: ArchiveFormat::Zip,
            root: None,
            server_file: expected_server_file(runtime).to_owned(),
        })
    } else {
        Err(RuntimeInstallError::UnsupportedArchive)
    }
}

fn open_verified_archive(
    path: &Path,
    asset: AssetSpec,
    cancellation: &CancellationToken,
) -> Result<File, RuntimeInstallError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    let metadata = file
        .metadata()
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    if !metadata.is_file() || metadata.len() != asset.byte_size() {
        return Err(RuntimeInstallError::ArchiveUnverified);
    }
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        if cancellation.is_cancelled() {
            return Err(RuntimeInstallError::Cancelled);
        }
        let count = file
            .read(&mut buffer)
            .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        hasher.update(&buffer[..count]);
    }
    if total != asset.byte_size() || format!("{:x}", hasher.finalize()) != asset.sha256() {
        return Err(RuntimeInstallError::ArchiveUnverified);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    Ok(file)
}

fn extract_archive(
    archive: File,
    destination: &Path,
    layout: &ArchiveLayout,
    cancellation: &CancellationToken,
) -> Result<Vec<InventoryFile>, RuntimeInstallError> {
    match layout.format {
        ArchiveFormat::TarGz => extract_tar_gz(archive, destination, layout, cancellation),
        ArchiveFormat::Zip => extract_zip(archive, destination, layout, cancellation),
    }
}

fn extract_tar_gz(
    archive: File,
    destination: &Path,
    layout: &ArchiveLayout,
    cancellation: &CancellationToken,
) -> Result<Vec<InventoryFile>, RuntimeInstallError> {
    let decoder = GzDecoder::new(archive);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|_| RuntimeInstallError::UnsafeArchive)?;
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut links = Vec::new();
    let mut expanded_bytes = 0_u64;
    let mut entry_count = 0_usize;

    for entry in entries {
        if cancellation.is_cancelled() {
            return Err(RuntimeInstallError::Cancelled);
        }
        entry_count += 1;
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(RuntimeInstallError::LimitsExceeded);
        }
        let mut entry = entry.map_err(|_| RuntimeInstallError::UnsafeArchive)?;
        let path_bytes = entry.path_bytes();
        let name =
            std::str::from_utf8(&path_bytes).map_err(|_| RuntimeInstallError::UnsafeArchive)?;
        let relative = portable_archive_path(name, layout.root.as_deref())?;
        let entry_type = entry.header().entry_type();
        let Some(relative) = relative else {
            if !entry_type.is_dir() {
                return Err(RuntimeInstallError::UnsafeArchive);
            }
            continue;
        };
        if !seen.insert(relative.clone()) {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        if entry_type.is_dir() {
            create_private_tree(destination, &relative)?;
        } else if entry_type.is_file() {
            let declared = entry
                .header()
                .size()
                .map_err(|_| RuntimeInstallError::UnsafeArchive)?;
            reserve_expanded(&mut expanded_bytes, declared)?;
            let is_server = relative == Path::new(&layout.server_file);
            files.push(write_archive_file(
                destination,
                &relative,
                &mut entry,
                declared,
                is_server,
                cancellation,
            )?);
        } else if entry_type.is_symlink() {
            let target = entry
                .link_name_bytes()
                .ok_or(RuntimeInstallError::UnsafeArchive)?;
            let target =
                std::str::from_utf8(&target).map_err(|_| RuntimeInstallError::UnsafeArchive)?;
            links.push(PendingLink {
                target: resolve_link_target(&relative, target)?,
                path: relative,
            });
        } else {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
    }
    materialize_links(
        destination,
        links,
        &mut files,
        &mut expanded_bytes,
        cancellation,
    )?;
    Ok(files)
}

fn extract_zip(
    archive: File,
    destination: &Path,
    layout: &ArchiveLayout,
    cancellation: &CancellationToken,
) -> Result<Vec<InventoryFile>, RuntimeInstallError> {
    let mut archive =
        zip::ZipArchive::new(archive).map_err(|_| RuntimeInstallError::UnsafeArchive)?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(RuntimeInstallError::LimitsExceeded);
    }
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut expanded_bytes = 0_u64;
    for index in 0..archive.len() {
        if cancellation.is_cancelled() {
            return Err(RuntimeInstallError::Cancelled);
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|_| RuntimeInstallError::UnsafeArchive)?;
        if entry.encrypted() || entry.is_symlink() || entry.enclosed_name().is_none() {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        if entry.unix_mode().is_some_and(|mode| {
            mode & 0o170000 != 0 && mode & 0o170000 != 0o100000 && !entry.is_dir()
        }) {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        let relative = portable_archive_path(entry.name(), layout.root.as_deref())?
            .ok_or(RuntimeInstallError::UnsafeArchive)?;
        if !seen.insert(relative.clone()) {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        if entry.is_dir() {
            create_private_tree(destination, &relative)?;
            continue;
        }
        if !entry.is_file() {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        let declared = entry.size();
        reserve_expanded(&mut expanded_bytes, declared)?;
        let is_server = relative == Path::new(&layout.server_file);
        files.push(write_archive_file(
            destination,
            &relative,
            &mut entry,
            declared,
            is_server,
            cancellation,
        )?);
    }
    Ok(files)
}

fn portable_archive_path(
    name: &str,
    expected_root: Option<&str>,
) -> Result<Option<PathBuf>, RuntimeInstallError> {
    if name.is_empty()
        || name.len() > MAX_PATH_BYTES
        || name.starts_with('/')
        || name.contains(['\\', '\0', ':'])
    {
        return Err(RuntimeInstallError::UnsafeArchive);
    }
    let name = name.trim_end_matches('/');
    let mut parts: Vec<_> = name.split('/').collect();
    if parts.iter().any(|part| !portable_component_is_safe(part)) {
        return Err(RuntimeInstallError::UnsafeArchive);
    }
    if let Some(root) = expected_root {
        if parts.first().copied() != Some(root) {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        parts.remove(0);
        if parts.is_empty() {
            return Ok(None);
        }
    }
    let mut result = PathBuf::new();
    for part in parts {
        result.push(part);
    }
    if result.as_os_str().is_empty() {
        Err(RuntimeInstallError::UnsafeArchive)
    } else {
        Ok(Some(result))
    }
}

fn portable_component_is_safe(component: &str) -> bool {
    if component.is_empty() || matches!(component, "." | "..") || component.ends_with([' ', '.']) {
        return false;
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

fn resolve_link_target(path: &Path, target: &str) -> Result<PathBuf, RuntimeInstallError> {
    let target = portable_archive_path(target, None)?.ok_or(RuntimeInstallError::UnsafeArchive)?;
    Ok(path.parent().unwrap_or_else(|| Path::new("")).join(target))
}

fn reserve_expanded(total: &mut u64, bytes: u64) -> Result<(), RuntimeInstallError> {
    if bytes > MAX_FILE_BYTES || bytes > MAX_EXPANDED_BYTES.saturating_sub(*total) {
        return Err(RuntimeInstallError::LimitsExceeded);
    }
    *total += bytes;
    Ok(())
}

fn write_archive_file(
    root: &Path,
    relative: &Path,
    reader: &mut impl Read,
    declared: u64,
    executable: bool,
    cancellation: &CancellationToken,
) -> Result<InventoryFile, RuntimeInstallError> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    create_private_tree(root, parent)?;
    let destination = root.join(relative);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    let mut output = options
        .open(&destination)
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    let mut hasher = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        if cancellation.is_cancelled() {
            return Err(RuntimeInstallError::Cancelled);
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|_| RuntimeInstallError::UnsafeArchive)?;
        if count == 0 {
            break;
        }
        if count as u64 > declared.saturating_sub(observed) {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
        hasher.update(&buffer[..count]);
        observed += count as u64;
    }
    if observed != declared {
        return Err(RuntimeInstallError::UnsafeArchive);
    }
    output
        .sync_all()
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    Ok(InventoryFile {
        path: portable_path_string(relative)?,
        bytes: observed,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn materialize_links(
    root: &Path,
    mut pending: Vec<PendingLink>,
    files: &mut Vec<InventoryFile>,
    expanded_bytes: &mut u64,
    cancellation: &CancellationToken,
) -> Result<(), RuntimeInstallError> {
    while !pending.is_empty() {
        let mut remaining = Vec::new();
        let mut progress = false;
        for link in pending {
            let source = root.join(&link.target);
            let metadata = match fs::symlink_metadata(&source) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    metadata
                }
                Ok(_) => return Err(RuntimeInstallError::UnsafeArchive),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    remaining.push(link);
                    continue;
                }
                Err(error) => return Err(RuntimeInstallError::Io(error.kind())),
            };
            reserve_expanded(expanded_bytes, metadata.len())?;
            let mut input =
                File::open(&source).map_err(|error| RuntimeInstallError::Io(error.kind()))?;
            files.push(write_archive_file(
                root,
                &link.path,
                &mut input,
                metadata.len(),
                false,
                cancellation,
            )?);
            progress = true;
        }
        if !progress {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        pending = remaining;
    }
    Ok(())
}

fn create_private_tree(root: &Path, relative: &Path) -> Result<(), RuntimeInstallError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(RuntimeInstallError::UnsafeArchive),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut builder = DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder
                    .create(&current)
                    .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
            }
            Err(error) => return Err(RuntimeInstallError::Io(error.kind())),
        }
        set_private_directory(&current)?;
    }
    Ok(())
}

fn set_private_directory(path: &Path) -> Result<(), RuntimeInstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_inventory(root: &Path, inventory: &RuntimeInventory) -> Result<(), RuntimeInstallError> {
    let mut bytes =
        serde_json::to_vec_pretty(inventory).map_err(|_| RuntimeInstallError::UnsafeArchive)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_INVENTORY_BYTES {
        return Err(RuntimeInstallError::LimitsExceeded);
    }
    let path = root.join(INVENTORY_FILE);
    let mut output = private_create_new(&path)?;
    output
        .write_all(&bytes)
        .and_then(|()| output.sync_all())
        .map_err(|error| RuntimeInstallError::Io(error.kind()))
}

fn verify_install_at(
    root: &Path,
    runtime: RuntimeAsset,
    level: VerificationLevel,
    cancellation: Option<&CancellationToken>,
) -> RuntimeInstallState {
    if cancellation.is_some_and(|token| token.is_cancelled()) {
        return RuntimeInstallState::Cancelled;
    }
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return RuntimeInstallState::Missing;
        }
        Err(_) => return RuntimeInstallState::Invalid,
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return RuntimeInstallState::Invalid;
    }
    let inventory = match read_inventory(root) {
        Ok(inventory) => inventory,
        Err(_) => return RuntimeInstallState::Invalid,
    };
    if inventory.schema != 1
        || inventory.manifest_id != MANIFEST_ID
        || inventory.asset_id != runtime.artifact().id()
        || inventory.archive_sha256 != runtime.artifact().sha256()
        || inventory.server_file != expected_server_file(runtime)
        || inventory.files.is_empty()
        || inventory.files.len() > MAX_ARCHIVE_ENTRIES
    {
        return RuntimeInstallState::Invalid;
    }

    let mut expected = BTreeMap::new();
    let mut expected_directories = BTreeSet::new();
    let mut expanded_bytes = 0_u64;
    for file in &inventory.files {
        if cancellation.is_some_and(|token| token.is_cancelled()) {
            return RuntimeInstallState::Cancelled;
        }
        let relative = match portable_archive_path(&file.path, None) {
            Ok(Some(relative)) => relative,
            _ => return RuntimeInstallState::Invalid,
        };
        if file.bytes > MAX_FILE_BYTES
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || file.bytes > MAX_EXPANDED_BYTES.saturating_sub(expanded_bytes)
        {
            return RuntimeInstallState::Invalid;
        }
        expanded_bytes += file.bytes;
        let mut parent = relative.parent();
        while let Some(directory) = parent {
            if !directory.as_os_str().is_empty() {
                expected_directories.insert(directory.to_path_buf());
            }
            parent = directory.parent();
        }
        if expected.insert(relative, file).is_some() {
            return RuntimeInstallState::Invalid;
        }
    }
    if !expected.contains_key(Path::new(expected_server_file(runtime))) {
        return RuntimeInstallState::Invalid;
    }

    let mut stack = vec![root.to_path_buf()];
    let mut observed_files = BTreeSet::new();
    while let Some(directory) = stack.pop() {
        if cancellation.is_some_and(|token| token.is_cancelled()) {
            return RuntimeInstallState::Cancelled;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => return RuntimeInstallState::Invalid,
        };
        for entry in entries {
            if cancellation.is_some_and(|token| token.is_cancelled()) {
                return RuntimeInstallState::Cancelled;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return RuntimeInstallState::Invalid,
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => return RuntimeInstallState::Invalid,
            };
            if metadata.file_type().is_symlink() {
                return RuntimeInstallState::Invalid;
            }
            let relative = match path.strip_prefix(root) {
                Ok(relative) => relative.to_path_buf(),
                Err(_) => return RuntimeInstallState::Invalid,
            };
            if metadata.is_dir() {
                if !expected_directories.contains(&relative) {
                    return RuntimeInstallState::Invalid;
                }
                stack.push(path);
            } else if metadata.is_file() {
                if relative == Path::new(INVENTORY_FILE) {
                    continue;
                }
                let Some(spec) = expected.get(&relative) else {
                    return RuntimeInstallState::Invalid;
                };
                if metadata.len() != spec.bytes || !observed_files.insert(relative.clone()) {
                    return RuntimeInstallState::Invalid;
                }
                #[cfg(unix)]
                if relative == Path::new(expected_server_file(runtime)) {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o111 == 0 {
                        return RuntimeInstallState::Invalid;
                    }
                }
                if level == VerificationLevel::Sha256 {
                    let hash = match hash_file(&path, spec.bytes, cancellation) {
                        Ok(hash) => hash,
                        Err(RuntimeInstallError::Cancelled) => {
                            return RuntimeInstallState::Cancelled;
                        }
                        Err(_) => return RuntimeInstallState::Invalid,
                    };
                    if hash != spec.sha256 {
                        return RuntimeInstallState::Invalid;
                    }
                }
            } else {
                return RuntimeInstallState::Invalid;
            }
        }
    }
    if cancellation.is_some_and(|token| token.is_cancelled()) {
        return RuntimeInstallState::Cancelled;
    }
    if observed_files.len() != expected.len() {
        return RuntimeInstallState::Invalid;
    }
    if level == VerificationLevel::Sha256 {
        RuntimeInstallState::Verified
    } else {
        RuntimeInstallState::PresentUnverified
    }
}

fn read_inventory(root: &Path) -> Result<RuntimeInventory, RuntimeInstallError> {
    let path = root.join(INVENTORY_FILE);
    let metadata =
        fs::symlink_metadata(&path).map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_INVENTORY_BYTES
    {
        return Err(RuntimeInstallError::UnsafeArchive);
    }
    let file = open_regular_read(&path)?;
    serde_json::from_reader(file).map_err(|_| RuntimeInstallError::UnsafeArchive)
}

fn hash_file(
    path: &Path,
    expected: u64,
    cancellation: Option<&CancellationToken>,
) -> Result<String, RuntimeInstallError> {
    if cancellation.is_some_and(|token| token.is_cancelled()) {
        return Err(RuntimeInstallError::Cancelled);
    }
    let mut file = open_regular_read(path)?;
    let mut hasher = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        if cancellation.is_some_and(|token| token.is_cancelled()) {
            return Err(RuntimeInstallError::Cancelled);
        }
        let count = file
            .read(&mut buffer)
            .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
        if count == 0 {
            break;
        }
        observed += count as u64;
        if observed > expected {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
        hasher.update(&buffer[..count]);
    }
    if cancellation.is_some_and(|token| token.is_cancelled()) {
        return Err(RuntimeInstallError::Cancelled);
    }
    if observed != expected {
        return Err(RuntimeInstallError::UnsafeArchive);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn open_regular_read(path: &Path) -> Result<File, RuntimeInstallError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    if !file
        .metadata()
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?
        .is_file()
    {
        return Err(RuntimeInstallError::UnsafeArchive);
    }
    Ok(file)
}

fn portable_path_string(path: &Path) -> Result<String, RuntimeInstallError> {
    let mut result = String::new();
    for component in path.components() {
        let value = component
            .as_os_str()
            .to_str()
            .ok_or(RuntimeInstallError::UnsafeArchive)?;
        if !result.is_empty() {
            result.push('/');
        }
        result.push_str(value);
    }
    Ok(result)
}

fn open_private_file(
    path: &Path,
    create: bool,
    truncate: bool,
) -> Result<File, RuntimeInstallError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RuntimeInstallError::UnsafeArchive);
        }
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    let metadata = file
        .metadata()
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    if !metadata.is_file() {
        return Err(RuntimeInstallError::UnsafeArchive);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    }
    Ok(file)
}

fn private_create_new(path: &Path) -> Result<File, RuntimeInstallError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .map_err(|error| RuntimeInstallError::Io(error.kind()))
}

fn sync_directory(path: &Path) -> Result<(), RuntimeInstallError> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| RuntimeInstallError::Io(error.kind()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Cursor;
    use tar::{Builder as TarBuilder, EntryType, Header};
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    fn paths(root: &Path) -> AppPaths {
        AppPaths::new(root.join("config"), root.join("data"), root.join("cache")).unwrap()
    }

    fn tar_file(entries: &[(&str, &[u8])], links: &[(&str, &str)], hard_link: bool) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = TarBuilder::new(encoder);
        for (name, body) in entries {
            let mut header = Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, *body).unwrap();
        }
        for (name, target) in links {
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::symlink());
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder.append_link(&mut header, name, target).unwrap();
        }
        if hard_link {
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::hard_link());
            header.set_size(0);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_link(&mut header, "llama-b10809/hard", "llama-b10809/lib.so")
                .unwrap();
        }
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    fn zip_file(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, body) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn runtime_with_archive(
        root: &Path,
        os: &'static str,
        file_name: &'static str,
        bytes: &[u8],
    ) -> (AppPaths, RuntimeAsset) {
        let app_paths = paths(root);
        let hash: &'static str = Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str());
        let asset = AssetSpec::test_fixture(
            crate::assets::AssetKind::RuntimeArchive,
            "runtime-test",
            file_name,
            bytes.len() as u64,
            hash,
        );
        let runtime = RuntimeAsset::test_fixture(os, "test", asset);
        let path = asset.cache_path(&app_paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        (app_paths, runtime)
    }

    #[test]
    fn consent_is_required_before_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let runtime = RuntimeAsset::test_fixture(
            "linux",
            "test",
            AssetSpec::test_fixture(
                crate::assets::AssetKind::RuntimeArchive,
                "runtime-test",
                "fixture.tar.gz",
                1,
                "00",
            ),
        );
        let result = install_cached_runtime(
            &app_paths,
            runtime,
            RuntimeInstallOptions { confirmed: false },
            &CancellationToken::new(),
        );
        assert_eq!(result, Err(RuntimeInstallError::ConsentRequired));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn tar_install_materializes_safe_links_and_verifies_inventory() {
        let bytes = tar_file(
            &[
                ("llama-b10809/llama-server", b"server"),
                ("llama-b10809/lib.so.1", b"library"),
            ],
            &[("llama-b10809/lib.so", "lib.so.1")],
            false,
        );
        let directory = tempfile::tempdir().unwrap();
        let (app_paths, runtime) =
            runtime_with_archive(directory.path(), "linux", "fixture.tar.gz", &bytes);
        let outcome = install_cached_runtime(
            &app_paths,
            runtime,
            RuntimeInstallOptions::confirmed(),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            RuntimeInstallOutcome::Installed { file_count: 3, .. }
        ));
        let inspection = inspect_installed_runtime(&app_paths, runtime, VerificationLevel::Sha256);
        assert_eq!(inspection.state(), RuntimeInstallState::Verified);
        assert_eq!(
            fs::read(inspection.path().join("lib.so")).unwrap(),
            b"library"
        );
        assert!(
            !fs::symlink_metadata(inspection.path().join("lib.so"))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(inspection.server_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn zip_install_uses_the_windows_server_name() {
        let bytes = zip_file(&[("llama-server.exe", b"server"), ("ggml.dll", b"library")]);
        let directory = tempfile::tempdir().unwrap();
        let (app_paths, runtime) =
            runtime_with_archive(directory.path(), "windows", "fixture.zip", &bytes);
        install_cached_runtime(
            &app_paths,
            runtime,
            RuntimeInstallOptions::confirmed(),
            &CancellationToken::new(),
        )
        .unwrap();
        let inspection = inspect_installed_runtime(&app_paths, runtime, VerificationLevel::Sha256);
        assert_eq!(inspection.state(), RuntimeInstallState::Verified);
        assert!(inspection.server_path().ends_with("llama-server.exe"));
    }

    #[test]
    fn missing_server_and_hard_links_fail_without_publication() {
        for bytes in [
            tar_file(&[("llama-b10809/lib.so", b"library")], &[], false),
            tar_file(
                &[
                    ("llama-b10809/llama-server", b"server"),
                    ("llama-b10809/lib.so", b"library"),
                ],
                &[],
                true,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let (app_paths, runtime) =
                runtime_with_archive(directory.path(), "linux", "fixture.tar.gz", &bytes);
            let result = install_cached_runtime(
                &app_paths,
                runtime,
                RuntimeInstallOptions::confirmed(),
                &CancellationToken::new(),
            );
            assert!(matches!(
                result,
                Err(RuntimeInstallError::ExpectedServerMissing | RuntimeInstallError::UnsafeArchive)
            ));
            assert_eq!(
                inspect_installed_runtime(&app_paths, runtime, VerificationLevel::MetadataOnly)
                    .state(),
                RuntimeInstallState::Missing
            );
        }
    }

    #[test]
    fn unsafe_link_target_is_rejected() {
        let bytes = tar_file(
            &[("llama-b10809/llama-server", b"server")],
            &[("llama-b10809/lib.so", "../outside")],
            false,
        );
        let directory = tempfile::tempdir().unwrap();
        let (app_paths, runtime) =
            runtime_with_archive(directory.path(), "linux", "fixture.tar.gz", &bytes);
        assert_eq!(
            install_cached_runtime(
                &app_paths,
                runtime,
                RuntimeInstallOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeInstallError::UnsafeArchive)
        );
    }

    #[test]
    fn installed_tree_tampering_and_extra_files_fail_verification() {
        let bytes = tar_file(&[("llama-b10809/llama-server", b"server")], &[], false);
        for extra in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let (app_paths, runtime) =
                runtime_with_archive(directory.path(), "linux", "fixture.tar.gz", &bytes);
            install_cached_runtime(
                &app_paths,
                runtime,
                RuntimeInstallOptions::confirmed(),
                &CancellationToken::new(),
            )
            .unwrap();
            let inspection =
                inspect_installed_runtime(&app_paths, runtime, VerificationLevel::Sha256);
            if extra {
                fs::write(inspection.path().join("extra"), b"extra").unwrap();
            } else {
                fs::write(inspection.server_path(), b"changed").unwrap();
            }
            assert_eq!(
                inspect_installed_runtime(&app_paths, runtime, VerificationLevel::Sha256).state(),
                RuntimeInstallState::Invalid
            );
        }
    }

    #[test]
    fn existing_invalid_install_is_never_overwritten() {
        let bytes = tar_file(&[("llama-b10809/llama-server", b"server")], &[], false);
        let directory = tempfile::tempdir().unwrap();
        let (app_paths, runtime) =
            runtime_with_archive(directory.path(), "linux", "fixture.tar.gz", &bytes);
        let path = install_path(&app_paths, runtime);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("owned"), b"keep").unwrap();
        assert_eq!(
            install_cached_runtime(
                &app_paths,
                runtime,
                RuntimeInstallOptions::confirmed(),
                &CancellationToken::new(),
            ),
            Err(RuntimeInstallError::ExistingInvalid)
        );
        assert_eq!(fs::read(path.join("owned")).unwrap(), b"keep");
    }

    #[test]
    fn portable_paths_reject_traversal_and_windows_special_names() {
        for name in [
            "../escape",
            "/absolute",
            "dir\\escape",
            "C:/escape",
            "dir/./file",
            "dir/../file",
            "CON",
            "aux.txt",
            "file. ",
        ] {
            assert_eq!(
                portable_archive_path(name, None),
                Err(RuntimeInstallError::UnsafeArchive),
                "{name}"
            );
        }
        assert_eq!(
            portable_archive_path("safe/nested/file", None).unwrap(),
            Some(PathBuf::from("safe/nested/file"))
        );
    }

    #[test]
    fn cancellable_installed_inspection_stops_on_token() {
        let bytes = tar_file(&[("llama-b10809/llama-server", b"server")], &[], false);
        let directory = tempfile::tempdir().unwrap();
        let (app_paths, runtime) =
            runtime_with_archive(directory.path(), "linux", "fixture.tar.gz", &bytes);
        install_cached_runtime(
            &app_paths,
            runtime,
            RuntimeInstallOptions::confirmed(),
            &CancellationToken::new(),
        )
        .unwrap();

        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let inspection = inspect_installed_runtime_cancellable(
            &app_paths,
            runtime,
            VerificationLevel::Sha256,
            Some(&cancellation),
        );
        assert_eq!(inspection.state(), RuntimeInstallState::Cancelled);
        assert!(inspection.state().is_cancelled());
        assert!(!inspection.state().is_verified());
    }
}
