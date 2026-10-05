use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::process::ExitCode;

use dociler_core::assets::{
    CacheState, LLAMA_CPP_BUILD, LLAMA_CPP_COMMIT, LLAMA_CPP_RELEASE, MANIFEST_ID, MODEL_ASSETS,
    ModelRemovalError, ModelRemoveTarget, VerificationLevel, current_runtime_asset,
    inspect_cached_asset_cancellable, preview_cached_removal, remove_cached_assets,
};
use dociler_core::chat::CancellationToken;
use dociler_core::config::{ConfigSource, ConfigStore, LoadedSettings, LocalProfile};
use dociler_core::credentials::{CredentialError, CredentialStore, OsCredentialStore, Secret};
use dociler_core::diagnostics::Diagnostics;
use dociler_core::downloads::{
    DownloadError, DownloadOptions, DownloadOutcome, DownloadProgress, download_cached_asset,
};
use dociler_core::hardware::{
    AcceleratorCandidate, HardwareInventory, MemoryScope, ModelPreflight, PreflightStatus,
};
use dociler_core::model_probe::{
    ModelProbeError, ModelProbeOptions, probe_cached_model_with_options,
};
use dociler_core::paths::AppPaths;
use dociler_core::profiles::{
    ProfileInstallError, ProfileMutationError, check_remote_profile, edit_remote_profile,
    install_remote_profile, remove_remote_profile, rotate_remote_credential,
};
use dociler_core::remote::{RemoteClient, RemoteError, RemoteProfile};
use dociler_core::runtime_install::{
    RuntimeInstallError, RuntimeInstallOptions, RuntimeInstallOutcome, RuntimeInstallState,
    inspect_installed_runtime, inspect_installed_runtime_cancellable, install_cached_runtime,
};
use dociler_core::runtime_probe::{
    RuntimeProbeError, RuntimeProbeOptions, probe_installed_runtime,
};
use dociler_core::session::{Role, Session};
use dociler_core::workspace::{DiscoveryError, Workspace, WritePolicy};

mod cli_signals;
mod tui;

const HELP: &str = "Dociler — local-first document assistant (development build)

Usage: dociler [COMMAND]

Commands:
  chat [NAME]   Open interactive terminal chat; optionally choose a profile
  doctor        Show basic workspace and platform diagnostics
  files [PATH]  List supported documents discovered in the workspace (no writes)
  config paths  Show OS config/model/runtime/cache locations (no writes)
  config show   Validate settings and show current-workspace policy (no writes)
  config init   Create safe default settings; never overwrite an existing file
  model status  Inspect RAM/CPU/disk and show Lite/Pro preflight results (no writes)
  model list    List pinned local assets and cache presence (no writes or hashing)
  model verify [PROFILE]
                Verify cached model/runtime sizes and SHA-256 (no writes)
  model download PROFILE --confirm [--restart]
                Download the pinned runtime and model; resume partial files by default
  model remove TARGET --confirm
                Safely remove a cached model (dociler-lite, dociler-pro), runtime, or all
  model repair PROFILE --confirm
                Inspect, clean, and re-download/reinstall corrupt or missing assets
  model runtime-install --confirm
                Safely extract and inventory the verified pinned runtime (never execute it)
  model runtime-probe --confirm
                Reverify, start a model-free loopback runtime, check health/auth/version, then stop it
  model load-probe PROFILE --confirm [--experimental] [--ceiling-bytes BYTES]
                Reverify a pinned GGUF/runtime, test a small CPU generation, then stop it
  connect list  List saved remote profiles (never credentials)
  connect verify URL MODEL
                Verify an endpoint without saving it
  connect add NAME URL MODEL
                Verify and save a new profile; reads optional DOCILER_API_KEY
  connect check NAME
                Re-verify a saved profile and its native credential
  connect edit NAME URL MODEL
                Verify and update endpoint/model; add --confirm-credential-destination
                before sending an existing key to a changed origin
  connect remove NAME --confirm
                Remove a profile and its OS-stored credential
  connect key NAME
                Verify and rotate a key from required DOCILER_API_KEY
  connect key-clear NAME --confirm
                Verify keyless access, then remove the OS-stored key
  run NAME       Stream one remote text response; reads the prompt from stdin
  help          Show this help

Options:
  -h, --help    Show this help
  -V, --version Show the build version

Interactive and one-shot remote text chat are available through saved profiles.
Document reading, persistent local model chat, and the Dociler API server are not available yet.
";

enum Command {
    Launch,
    Help,
    Version,
    Doctor,
    ConfigPaths,
    ConfigShow,
    ConfigInit,
    ModelStatus,
    ModelList,
    ModelVerify {
        profile: Option<LocalProfile>,
    },
    ModelDownload {
        profile: LocalProfile,
        confirmed: bool,
        restart_partial: bool,
    },
    ModelRemove {
        target: ModelRemoveTarget,
        confirmed: bool,
    },
    ModelRepair {
        profile: LocalProfile,
        confirmed: bool,
    },
    ModelRuntimeInstall {
        confirmed: bool,
    },
    ModelRuntimeProbe {
        confirmed: bool,
    },
    ModelLoadProbe {
        profile: LocalProfile,
        confirmed: bool,
        allow_experimental: bool,
        memory_ceiling_bytes: Option<u64>,
    },
    ConnectList,
    ConnectVerify {
        url: String,
        model: String,
    },
    ConnectAdd {
        name: String,
        url: String,
        model: String,
    },
    ConnectRemove {
        name: String,
        confirmed: bool,
    },
    ConnectCheck {
        name: String,
    },
    ConnectEdit {
        name: String,
        url: String,
        model: String,
        confirmed_destination: bool,
    },
    ConnectKey {
        name: String,
        clear: bool,
        confirmed: bool,
    },
    Run {
        name: String,
    },
    Chat {
        name: Option<String>,
    },
    Files {
        path: Option<std::path::PathBuf>,
    },
}

fn parse(args: &[OsString]) -> Option<Command> {
    match args {
        [] => Some(Command::Launch),
        [arg] if arg == "help" || arg == "--help" || arg == "-h" => Some(Command::Help),
        [arg] if arg == "--version" || arg == "-V" => Some(Command::Version),
        [arg] if arg == "doctor" => Some(Command::Doctor),
        [command] if command == "files" => Some(Command::Files { path: None }),
        [command, path] if command == "files" && path != "--help" && path != "-h" => {
            Some(Command::Files {
                path: Some(std::path::PathBuf::from(path)),
            })
        }
        [command, flag]
            if (command == "doctor"
                || command == "config"
                || command == "model"
                || command == "files")
                && (flag == "--help" || flag == "-h") =>
        {
            Some(Command::Help)
        }
        [command, action] if command == "config" && action == "paths" => Some(Command::ConfigPaths),
        [command, action] if command == "config" && action == "show" => Some(Command::ConfigShow),
        [command, action] if command == "config" && action == "init" => Some(Command::ConfigInit),
        [command, action] if command == "model" && action == "status" => Some(Command::ModelStatus),
        [command, action] if command == "model" && action == "list" => Some(Command::ModelList),
        [command, action] if command == "model" && action == "verify" => {
            Some(Command::ModelVerify { profile: None })
        }
        [command, action, profile] if command == "model" && action == "verify" => {
            let profile = match profile.to_str()? {
                "dociler-lite" => LocalProfile::Lite,
                "dociler-pro" => LocalProfile::Pro,
                _ => return None,
            };
            Some(Command::ModelVerify {
                profile: Some(profile),
            })
        }
        [command, action, profile] if command == "model" && action == "download" => {
            Some(Command::ModelDownload {
                profile: parse_local_profile(profile)?,
                confirmed: false,
                restart_partial: false,
            })
        }
        [command, action, profile, flag]
            if command == "model" && action == "download" && flag == "--confirm" =>
        {
            Some(Command::ModelDownload {
                profile: parse_local_profile(profile)?,
                confirmed: true,
                restart_partial: false,
            })
        }
        [command, action, profile, first, second]
            if command == "model"
                && action == "download"
                && ((first == "--confirm" && second == "--restart")
                    || (first == "--restart" && second == "--confirm")) =>
        {
            Some(Command::ModelDownload {
                profile: parse_local_profile(profile)?,
                confirmed: true,
                restart_partial: true,
            })
        }
        [command, action, target] if command == "model" && action == "remove" => {
            Some(Command::ModelRemove {
                target: parse_model_remove_target(target)?,
                confirmed: false,
            })
        }
        [command, action, target, flag]
            if command == "model" && action == "remove" && flag == "--confirm" =>
        {
            Some(Command::ModelRemove {
                target: parse_model_remove_target(target)?,
                confirmed: true,
            })
        }
        [command, action, profile] if command == "model" && action == "repair" => {
            Some(Command::ModelRepair {
                profile: parse_local_profile(profile)?,
                confirmed: false,
            })
        }
        [command, action, profile, flag]
            if command == "model" && action == "repair" && flag == "--confirm" =>
        {
            Some(Command::ModelRepair {
                profile: parse_local_profile(profile)?,
                confirmed: true,
            })
        }
        [command, action] if command == "model" && action == "runtime-install" => {
            Some(Command::ModelRuntimeInstall { confirmed: false })
        }
        [command, action, flag]
            if command == "model" && action == "runtime-install" && flag == "--confirm" =>
        {
            Some(Command::ModelRuntimeInstall { confirmed: true })
        }
        [command, action] if command == "model" && action == "runtime-probe" => {
            Some(Command::ModelRuntimeProbe { confirmed: false })
        }
        [command, action, flag]
            if command == "model" && action == "runtime-probe" && flag == "--confirm" =>
        {
            Some(Command::ModelRuntimeProbe { confirmed: true })
        }
        [command, action, profile, rest @ ..] if command == "model" && action == "load-probe" => {
            let profile = parse_local_profile(profile)?;
            let mut confirmed = false;
            let mut allow_experimental = false;
            let mut memory_ceiling_bytes = None;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                let s = arg.to_str()?;
                if s == "--confirm" {
                    confirmed = true;
                } else if s == "--experimental" || s == "--allow-experimental" {
                    allow_experimental = true;
                } else if s == "--ceiling-bytes" || s == "--memory-ceiling-bytes" {
                    let val = iter.next()?.to_str()?;
                    let bytes: u64 = val.parse().ok()?;
                    if bytes == 0 {
                        return None;
                    }
                    memory_ceiling_bytes = Some(bytes);
                } else if let Some(val) = s
                    .strip_prefix("--ceiling-bytes=")
                    .or_else(|| s.strip_prefix("--memory-ceiling-bytes="))
                {
                    let bytes: u64 = val.parse().ok()?;
                    if bytes == 0 {
                        return None;
                    }
                    memory_ceiling_bytes = Some(bytes);
                } else {
                    return None;
                }
            }
            Some(Command::ModelLoadProbe {
                profile,
                confirmed,
                allow_experimental,
                memory_ceiling_bytes,
            })
        }
        [command, action] if command == "connect" && action == "list" => Some(Command::ConnectList),
        [command, action, url, model] if command == "connect" && action == "verify" => {
            Some(Command::ConnectVerify {
                url: url.to_str()?.to_owned(),
                model: model.to_str()?.to_owned(),
            })
        }
        [command, action, name, url, model] if command == "connect" && action == "add" => {
            Some(Command::ConnectAdd {
                name: name.to_str()?.to_owned(),
                url: url.to_str()?.to_owned(),
                model: model.to_str()?.to_owned(),
            })
        }
        [command, action, name] if command == "connect" && action == "check" => {
            Some(Command::ConnectCheck {
                name: name.to_str()?.to_owned(),
            })
        }
        [command, action, name, url, model] if command == "connect" && action == "edit" => {
            Some(Command::ConnectEdit {
                name: name.to_str()?.to_owned(),
                url: url.to_str()?.to_owned(),
                model: model.to_str()?.to_owned(),
                confirmed_destination: false,
            })
        }
        [command, action, name, url, model, flag]
            if command == "connect"
                && action == "edit"
                && flag == "--confirm-credential-destination" =>
        {
            Some(Command::ConnectEdit {
                name: name.to_str()?.to_owned(),
                url: url.to_str()?.to_owned(),
                model: model.to_str()?.to_owned(),
                confirmed_destination: true,
            })
        }
        [command, action, name] if command == "connect" && action == "remove" => {
            Some(Command::ConnectRemove {
                name: name.to_str()?.to_owned(),
                confirmed: false,
            })
        }
        [command, action, name, flag]
            if command == "connect" && action == "remove" && flag == "--confirm" =>
        {
            Some(Command::ConnectRemove {
                name: name.to_str()?.to_owned(),
                confirmed: true,
            })
        }
        [command, action, name] if command == "connect" && action == "key" => {
            Some(Command::ConnectKey {
                name: name.to_str()?.to_owned(),
                clear: false,
                confirmed: true,
            })
        }
        [command, action, name, flag]
            if command == "connect" && action == "key-clear" && flag == "--confirm" =>
        {
            Some(Command::ConnectKey {
                name: name.to_str()?.to_owned(),
                clear: true,
                confirmed: true,
            })
        }
        [command, action, name] if command == "connect" && action == "key-clear" => {
            Some(Command::ConnectKey {
                name: name.to_str()?.to_owned(),
                clear: true,
                confirmed: false,
            })
        }
        [command, name] if command == "run" => Some(Command::Run {
            name: name.to_str()?.to_owned(),
        }),
        [command] if command == "chat" => Some(Command::Chat { name: None }),
        [command, name] if command == "chat" => Some(Command::Chat {
            name: Some(name.to_str()?.to_owned()),
        }),
        _ => None,
    }
}

fn parse_local_profile(value: &OsString) -> Option<LocalProfile> {
    match value.to_str()? {
        "dociler-lite" => Some(LocalProfile::Lite),
        "dociler-pro" => Some(LocalProfile::Pro),
        _ => None,
    }
}

fn parse_model_remove_target(value: &OsString) -> Option<ModelRemoveTarget> {
    match value.to_str()? {
        "dociler-lite" => Some(ModelRemoveTarget::Profile(LocalProfile::Lite)),
        "dociler-pro" => Some(ModelRemoveTarget::Profile(LocalProfile::Pro)),
        "runtime" => Some(ModelRemoveTarget::Runtime),
        "all" => Some(ModelRemoveTarget::All),
        _ => None,
    }
}

enum CommandError {
    Output(io::Error),
    Workspace,
    Discovery(DiscoveryError),
    Config(io::ErrorKind),
    Credential(CredentialError),
    Input,
    Usage(&'static str),
    Conflict,
    AssetVerification,
    AssetDownload(DownloadError),
    AssetRemoval(ModelRemovalError),
    RuntimeInstall(RuntimeInstallError),
    RuntimeProbe(RuntimeProbeError),
    ModelProbe(ModelProbeError),
    Signal,
    Cancelled,
    Remote(RemoteError),
    Terminal,
}

impl From<io::Error> for CommandError {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
    }
}

impl From<DiscoveryError> for CommandError {
    fn from(error: DiscoveryError) -> Self {
        Self::Discovery(error)
    }
}

fn paths() -> Result<AppPaths, CommandError> {
    AppPaths::discover().map_err(|error| CommandError::Config(error.kind()))
}

fn settings() -> Result<LoadedSettings, CommandError> {
    ConfigStore::new(paths()?)
        .load()
        .map_err(|error| CommandError::Config(error.kind()))
}

fn workspace() -> Result<Workspace, CommandError> {
    let cwd = std::env::current_dir().map_err(|_| CommandError::Workspace)?;
    Workspace::open(&cwd).map_err(|_| CommandError::Workspace)
}

fn print_settings(
    output: &mut impl Write,
    loaded: &LoadedSettings,
    workspace: &Workspace,
) -> io::Result<()> {
    writeln!(
        output,
        "Configuration: {}",
        match loaded.source {
            ConfigSource::Defaults => "defaults (no saved file)",
            ConfigSource::Saved => "saved, schema v1 (validated)",
        }
    )?;
    writeln!(
        output,
        "Preferred local profile: {} (not loaded)",
        loaded.settings.preferred_local_profile().alias()
    )?;
    writeln!(
        output,
        "Workspace access: {}",
        match loaded.settings.write_policy(workspace) {
            WritePolicy::ReadOnly => "read-only",
            WritePolicy::ConfirmEveryWrite =>
                "grant recorded; each write requires confirmation (editing not implemented)",
        }
    )?;
    writeln!(output, "Session storage: memory only; no persisted history")?;
    writeln!(
        output,
        "Credential store: native OS adapter (availability checked only when used)"
    )?;
    writeln!(
        output,
        "Saved remote profiles: {}",
        loaded.settings.remote_profiles().len()
    )
}

fn gibibytes(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn optional_gibibytes(bytes: Option<u64>) -> String {
    bytes.map(gibibytes).unwrap_or_else(|| "unknown".to_owned())
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
}

fn preflight_label(status: PreflightStatus) -> &'static str {
    match status {
        PreflightStatus::ReadyForRuntimeProbe => "ready for required runtime probe",
        PreflightStatus::ExperimentalForRuntimeProbe => {
            "experimental RAM class; ready for required runtime probe"
        }
        PreflightStatus::InsufficientTotalMemory => "unsupported total RAM; use Remote mode",
        PreflightStatus::InsufficientAvailableMemory => {
            "not enough memory available now; close workloads or use Remote mode"
        }
        PreflightStatus::InsufficientDisk => {
            "not enough free asset-storage space; free disk or use Remote mode"
        }
        PreflightStatus::InventoryIncomplete => {
            "inventory incomplete; local mode remains unavailable"
        }
    }
}

fn print_hardware(output: &mut impl Write, hardware: &HardwareInventory) -> io::Result<()> {
    writeln!(output, "Hardware inventory: read-only; not persisted")?;
    writeln!(
        output,
        "Memory: {} total, {} available ({})",
        optional_gibibytes(hardware.total_memory_bytes()),
        optional_gibibytes(hardware.available_memory_bytes()),
        match hardware.memory_scope() {
            MemoryScope::Host => "host",
            MemoryScope::Cgroup => "container/cgroup limit",
        }
    )?;
    writeln!(
        output,
        "CPU: {} logical available, {} physical; planned inference threads: {}",
        hardware.logical_cpu_count(),
        hardware
            .physical_cpu_count()
            .map(|count| count.to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
        hardware.recommended_threads()
    )?;
    writeln!(
        output,
        "CPU features: {}",
        if hardware.cpu_features().is_empty() {
            "baseline/none reported".to_owned()
        } else {
            hardware.cpu_features().join(", ")
        }
    )?;
    if let Some(runtime) = current_runtime_asset() {
        let cpu_check = hardware.evaluate_cpu_instructions(*runtime);
        if cpu_check.supported {
            writeln!(
                output,
                "Runtime CPU instructions: satisfied (required: {})",
                if runtime.required_cpu_features().is_empty() {
                    "none".to_owned()
                } else {
                    runtime.required_cpu_features().join(", ")
                }
            )?;
        } else {
            writeln!(
                output,
                "Runtime CPU instructions: missing required: {}",
                cpu_check.missing_required.join(", ")
            )?;
        }
    }
    writeln!(
        output,
        "Asset storage: {} free{}",
        optional_gibibytes(hardware.free_disk_bytes()),
        hardware
            .disk_mount()
            .map(|mount| format!(" on {mount:?}"))
            .unwrap_or_default()
    )?;
    writeln!(
        output,
        "Accelerator: {}",
        if hardware
            .accelerator_candidates()
            .contains(&AcceleratorCandidate::Metal)
        {
            "Metal candidate; not selected until a packaged-runtime probe passes"
        } else {
            "no candidate reported yet; CUDA/Vulkan discovery is not implemented"
        }
    )?;
    writeln!(output, "Local model preflight:")?;
    for profile in [LocalProfile::Lite, LocalProfile::Pro] {
        let report = ModelPreflight::evaluate(profile, hardware);
        let requirements = report.requirements();
        writeln!(
            output,
            "  {}: {}",
            profile.alias(),
            preflight_label(report.status())
        )?;
        writeln!(
            output,
            "    target RAM {:.0} GB class, current available floor {}, disk floor {}, context {} tokens; model download about {}",
            requirements.supported_total_memory_bytes() as f64 / 1_000_000_000.0,
            gibibytes(requirements.required_available_memory_bytes()),
            gigabytes(requirements.required_free_disk_bytes()),
            requirements.context_tokens(),
            gigabytes(requirements.approximate_model_bytes())
        )?;
    }
    writeln!(
        output,
        "Preflight is not final admission: no runtime/model was downloaded or executed."
    )
}

fn cache_state_label(state: &CacheState) -> String {
    match state {
        CacheState::Missing => "missing".to_owned(),
        CacheState::PresentUnverified => "present; checksum not run".to_owned(),
        CacheState::Verified => "verified".to_owned(),
        CacheState::SizeMismatch { expected, observed } => {
            format!("invalid size: expected {expected} bytes, observed {observed}")
        }
        CacheState::HashMismatch { .. } => "invalid SHA-256".to_owned(),
        CacheState::UnsafeFileType => "unsafe path or file type".to_owned(),
        CacheState::Unreadable(_) => "unreadable".to_owned(),
        CacheState::Cancelled => "cancelled".to_owned(),
    }
}

fn print_model_cache(
    output: &mut impl Write,
    app_paths: &AppPaths,
    profile: LocalProfile,
    level: VerificationLevel,
    cancellation: Option<&CancellationToken>,
) -> io::Result<bool> {
    let model = dociler_core::assets::model_asset(profile);
    let artifact = model.artifact();
    let inspection = inspect_cached_asset_cancellable(app_paths, artifact, level, cancellation);
    writeln!(
        output,
        "{}: {} {} Q4_K_M, {} bytes, cache={}",
        profile.alias(),
        model.base_model(),
        model.license(),
        artifact.byte_size(),
        cache_state_label(inspection.state())
    )?;
    writeln!(
        output,
        "  upstream={}@{}",
        model.repository(),
        model.revision()
    )?;
    writeln!(output, "  source={}", artifact.source_url())?;
    writeln!(output, "  sha256={}", artifact.sha256())?;
    writeln!(output, "  file={:?}", inspection.path())?;
    Ok(inspection.state().is_verified())
}

fn print_runtime_cache(
    output: &mut impl Write,
    app_paths: &AppPaths,
    level: VerificationLevel,
    cancellation: Option<&CancellationToken>,
) -> io::Result<bool> {
    let Some(runtime) = current_runtime_asset() else {
        writeln!(
            output,
            "llama.cpp runtime: unsupported manifest target {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )?;
        return Ok(false);
    };
    let inspection =
        inspect_cached_asset_cancellable(app_paths, runtime.artifact(), level, cancellation);
    writeln!(
        output,
        "llama.cpp runtime: {LLAMA_CPP_RELEASE}/{LLAMA_CPP_BUILD} {}-{} {}, cache={}",
        runtime.operating_system(),
        runtime.architecture(),
        runtime.backend(),
        cache_state_label(inspection.state())
    )?;
    writeln!(output, "  source={}", runtime.artifact().source_url())?;
    writeln!(output, "  sha256={}", runtime.artifact().sha256())?;
    writeln!(output, "  file={:?}", inspection.path())?;
    writeln!(
        output,
        "  required-cpu={}",
        if runtime.required_cpu_features().is_empty() {
            "none".to_owned()
        } else {
            runtime.required_cpu_features().join(", ")
        }
    )?;
    writeln!(
        output,
        "  expected-libraries={}",
        runtime.expected_shared_libraries().join(", ")
    )?;
    let installed = inspect_installed_runtime_cancellable(app_paths, *runtime, level, cancellation);
    let installed_state = installed.state();
    writeln!(
        output,
        "  installed={} directory={:?} server={:?}",
        match installed_state {
            RuntimeInstallState::Missing => "missing",
            RuntimeInstallState::PresentUnverified => "present; checksums not run",
            RuntimeInstallState::Verified => "verified",
            RuntimeInstallState::Invalid => "invalid",
            RuntimeInstallState::Cancelled => "cancelled",
        },
        installed.path(),
        installed.server_path()
    )?;
    Ok(inspection.state().is_verified()
        && installed_state != RuntimeInstallState::Invalid
        && installed_state != RuntimeInstallState::Cancelled)
}

fn print_asset_manifest_header(output: &mut impl Write) -> io::Result<()> {
    writeln!(
        output,
        "Asset manifest: {MANIFEST_ID} (built into this executable)"
    )?;
    writeln!(
        output,
        "Runtime pin: llama.cpp {LLAMA_CPP_RELEASE}, build {LLAMA_CPP_BUILD}, commit {LLAMA_CPP_COMMIT}"
    )
}

fn download_asset(
    output: &mut impl Write,
    app_paths: &AppPaths,
    label: &str,
    license: &str,
    artifact: dociler_core::assets::AssetSpec,
    restart_partial: bool,
    cancellation: &CancellationToken,
) -> Result<(), CommandError> {
    writeln!(output, "Downloading {label}:")?;
    writeln!(output, "  license={license}")?;
    writeln!(output, "  source={}", artifact.source_url())?;
    writeln!(output, "  bytes={}", artifact.byte_size())?;
    writeln!(output, "  sha256={}", artifact.sha256())?;
    writeln!(output, "  destination={:?}", artifact.cache_path(app_paths))?;
    output.flush()?;

    let mut last_percent = None;
    let mut progress_error = None;
    let outcome = download_cached_asset(
        app_paths,
        artifact,
        DownloadOptions::confirmed(restart_partial),
        cancellation,
        |DownloadProgress {
             downloaded_bytes,
             total_bytes,
             resumed_from,
         }| {
            let percent = downloaded_bytes.saturating_mul(100) / total_bytes.max(1);
            if last_percent.is_none_or(|previous| percent >= previous + 5 || percent == 100) {
                if let Err(error) = writeln!(
                    output,
                    "  progress={percent}% ({downloaded_bytes}/{total_bytes} bytes, resumed-from={resumed_from})"
                ) {
                    progress_error = Some(error);
                }
                last_percent = Some(percent);
            }
        },
    );
    if let Some(error) = progress_error {
        return Err(CommandError::Output(error));
    }
    match outcome.map_err(CommandError::AssetDownload)? {
        DownloadOutcome::AlreadyVerified => writeln!(output, "  result=already verified")?,
        DownloadOutcome::Published {
            resumed_from,
            partial_cleanup_warning,
        } => {
            writeln!(
                output,
                "  result=verified and published (resumed-from={resumed_from})"
            )?;
            if partial_cleanup_warning {
                writeln!(
                    output,
                    "  warning=verified final asset is ready, but its partial sibling could not be removed"
                )?;
            }
        }
    }
    Ok(())
}

fn environment_secret() -> Result<Option<Secret>, CommandError> {
    match std::env::var("DOCILER_API_KEY") {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) if value.len() <= 16 * 1024 => Ok(Some(Secret::new(value))),
        Ok(_) | Err(std::env::VarError::NotUnicode(_)) => Err(CommandError::Input),
        Err(std::env::VarError::NotPresent) => Ok(None),
    }
}

fn secret_for(profile: &RemoteProfile) -> Result<Option<Secret>, CommandError> {
    if !profile.needs_credential() {
        return Ok(None);
    }
    OsCredentialStore
        .get(&profile.credential_id())
        .map_err(CommandError::Credential)?
        .ok_or(CommandError::Credential(CredentialError::Unavailable))
        .map(Some)
}

fn remote_profile(name: &str) -> Result<RemoteProfile, CommandError> {
    settings()?
        .settings
        .remote_profile(name)
        .cloned()
        .ok_or(CommandError::Input)
}

fn map_profile_mutation(error: ProfileMutationError) -> CommandError {
    match error {
        ProfileMutationError::Config(kind) => CommandError::Config(kind),
        ProfileMutationError::Credential(error)
        | ProfileMutationError::CredentialRollback(error) => CommandError::Credential(error),
        ProfileMutationError::Remote(error) => CommandError::Remote(error),
        ProfileMutationError::Cancelled => CommandError::Remote(RemoteError::Cancelled),
        ProfileMutationError::NotFound => CommandError::Input,
        ProfileMutationError::Conflict => CommandError::Conflict,
        ProfileMutationError::DestinationConsentRequired => CommandError::Usage(
            "editing an authenticated profile to a new origin requires --confirm-credential-destination; no key was sent and nothing changed",
        ),
    }
}

fn interactive(name: Option<&str>) -> Result<(), CommandError> {
    let workspace = workspace()?;
    let app_paths = paths()?;
    let loaded = ConfigStore::new(app_paths.clone())
        .load()
        .map_err(|error| CommandError::Config(error.kind()))?;
    if name.is_some_and(|name| loaded.settings.remote_profile(name).is_none()) {
        return Err(CommandError::Input);
    }
    tui::run(
        workspace,
        app_paths,
        loaded.settings.remote_profiles().to_vec(),
        name,
    )
    .map_err(|_| CommandError::Terminal)
}

fn execute(command: Command, output: &mut impl Write) -> Result<(), CommandError> {
    match command {
        Command::Launch | Command::Chat { .. } => {
            unreachable!("interactive commands are handled before execute")
        }
        Command::Help => write!(output, "{HELP}")?,
        Command::Version => writeln!(output, "dociler {}", env!("CARGO_PKG_VERSION"))?,
        Command::Files { path } => {
            let workspace = match path {
                Some(p) => Workspace::open(&p).map_err(|err| {
                    let _ = writeln!(
                        io::stderr().lock(),
                        "Failed to open workspace at '{}': {err}",
                        p.display()
                    );
                    CommandError::Workspace
                })?,
                None => workspace()?,
            };

            let report = workspace
                .discover_documents()
                .map_err(CommandError::Discovery)?;

            writeln!(output, "Workspace: {}", workspace.root().display())?;

            if report.documents.is_empty() {
                writeln!(
                    output,
                    "No supported documents discovered (Markdown, Plain Text, PDF, DOCX, DOC, RTF, ODT)."
                )?;
            } else {
                writeln!(
                    output,
                    "Discovered {} document(s) ({}):",
                    report.documents.len(),
                    format_cli_bytes(report.total_document_bytes)
                )?;
                writeln!(output)?;
                for doc in &report.documents {
                    writeln!(
                        output,
                        "  {} ({}, {}){}",
                        doc.relative_path.display(),
                        doc.format,
                        format_cli_bytes(doc.byte_size),
                        if doc.is_direct_editable() {
                            " [direct-editable]"
                        } else {
                            ""
                        }
                    )?;
                }
            }

            if report.ignored_files_count > 0
                || report.skipped_symlinks_count > 0
                || report.skipped_oversized_count > 0
            {
                writeln!(output)?;
                writeln!(output, "Discovery filters:")?;
                if report.ignored_files_count > 0 {
                    writeln!(
                        output,
                        "  {} file(s) ignored by .gitignore or exclusions",
                        report.ignored_files_count
                    )?;
                }
                if report.skipped_symlinks_count > 0 {
                    writeln!(
                        output,
                        "  {} symlink(s) skipped (directory or escaping target)",
                        report.skipped_symlinks_count
                    )?;
                }
                if report.skipped_oversized_count > 0 {
                    writeln!(
                        output,
                        "  {} oversized document(s) skipped (> 50 MiB)",
                        report.skipped_oversized_count
                    )?;
                }
            }
        }
        Command::ConfigPaths => {
            let paths = paths()?;
            writeln!(output, "Config file: {:?}", paths.config_file())?;
            writeln!(output, "Model directory: {:?}", paths.models_dir())?;
            writeln!(output, "Runtime directory: {:?}", paths.runtimes_dir())?;
            writeln!(output, "Cache directory: {:?}", paths.cache_dir)?;
            writeln!(
                output,
                "Paths only; no directories created. No document or session cache is written."
            )?;
        }
        Command::ConfigShow => {
            let workspace = workspace()?;
            let loaded = settings()?;
            writeln!(output, "Workspace: {:?}", workspace.root())?;
            print_settings(output, &loaded, &workspace)?;
        }
        Command::ConfigInit => {
            let paths = paths()?;
            let path = paths.config_file();
            ConfigStore::new(paths)
                .initialize()
                .map_err(|error| CommandError::Config(error.kind()))?;
            writeln!(output, "Created defaults at {path:?}")?;
            writeln!(
                output,
                "All workspaces remain read-only. No models downloaded or services started."
            )?;
        }
        Command::ModelStatus => {
            let app_paths = paths()?;
            let hardware = HardwareInventory::inspect(&app_paths.models_dir());
            print_hardware(output, &hardware)?;
        }
        Command::ModelList => {
            let app_paths = paths()?;
            print_asset_manifest_header(output)?;
            for model in MODEL_ASSETS {
                print_model_cache(
                    output,
                    &app_paths,
                    model.profile(),
                    VerificationLevel::MetadataOnly,
                    None,
                )?;
            }
            print_runtime_cache(output, &app_paths, VerificationLevel::MetadataOnly, None)?;
            writeln!(
                output,
                "Read-only metadata inspection only; run 'dociler model verify [PROFILE]' for SHA-256 verification."
            )?;
        }
        Command::ModelVerify { profile } => {
            let app_paths = paths()?;
            let cancellation = CancellationToken::new();
            let _signals = cli_signals::CliSignalGuard::install(cancellation.clone())
                .map_err(|_| CommandError::Signal)?;
            print_asset_manifest_header(output)?;
            let profiles: &[LocalProfile] = match profile {
                Some(LocalProfile::Lite) => &[LocalProfile::Lite],
                Some(LocalProfile::Pro) => &[LocalProfile::Pro],
                None => &[LocalProfile::Lite, LocalProfile::Pro],
            };
            let mut verified = true;
            for profile in profiles {
                if cancellation.is_cancelled() {
                    break;
                }
                verified &= print_model_cache(
                    output,
                    &app_paths,
                    *profile,
                    VerificationLevel::Sha256,
                    Some(&cancellation),
                )?;
            }
            if !cancellation.is_cancelled() {
                verified &= print_runtime_cache(
                    output,
                    &app_paths,
                    VerificationLevel::Sha256,
                    Some(&cancellation),
                )?;
            }
            if cancellation.is_cancelled() {
                writeln!(output, "Verification was cancelled.")?;
                return Err(CommandError::Cancelled);
            }
            writeln!(
                output,
                "Verification was read-only; no asset was changed or executed."
            )?;
            if !verified {
                return Err(CommandError::AssetVerification);
            }
        }
        Command::ModelDownload {
            profile,
            confirmed,
            restart_partial,
        } => {
            if !confirmed {
                return Err(CommandError::Usage(
                    "model download requires explicit consent: dociler model download PROFILE --confirm [--restart]",
                ));
            }
            let app_paths = paths()?;
            let runtime = current_runtime_asset().ok_or(CommandError::Usage(
                "no pinned llama.cpp runtime exists for this platform; use Remote mode",
            ))?;
            let model = dociler_core::assets::model_asset(profile);
            let cancellation = CancellationToken::new();
            let _signals = cli_signals::CliSignalGuard::install(cancellation.clone())
                .map_err(|_| CommandError::Signal)?;
            print_asset_manifest_header(output)?;
            writeln!(
                output,
                "Consent recorded for these two immutable artifacts. No document or credential will be transmitted."
            )?;
            download_asset(
                output,
                &app_paths,
                "llama.cpp runtime archive",
                "MIT",
                runtime.artifact(),
                restart_partial,
                &cancellation,
            )?;
            download_asset(
                output,
                &app_paths,
                profile.alias(),
                model.license(),
                model.artifact(),
                restart_partial,
                &cancellation,
            )?;
            writeln!(
                output,
                "Assets are cached and verified only; archive extraction and local execution remain disabled."
            )?;
        }
        Command::ModelRemove { target, confirmed } => {
            let app_paths = paths()?;
            let preview =
                preview_cached_removal(&app_paths, target).map_err(CommandError::AssetRemoval)?;
            if !confirmed {
                writeln!(output, "Previewing cache removal for {}:", target.label())?;
                if preview.removed_paths.is_empty() {
                    writeln!(output, "  No cached files found; 0 bytes to free.")?;
                } else {
                    for path in &preview.removed_paths {
                        writeln!(output, "  candidate={path:?}")?;
                    }
                    writeln!(output, "  potential-freed-bytes={}", preview.bytes_freed)?;
                }
                writeln!(
                    output,
                    "To execute removal, run: dociler model remove {} --confirm",
                    target.label()
                )?;
                return Err(CommandError::Usage(
                    "model removal requires explicit consent: dociler model remove TARGET --confirm",
                ));
            }
            let outcome = remove_cached_assets(&app_paths, target, true)
                .map_err(CommandError::AssetRemoval)?;
            writeln!(
                output,
                "Removed {} cached asset file(s) for {}, freeing {} bytes.",
                outcome.removed_paths.len(),
                outcome.target.label(),
                outcome.bytes_freed
            )?;
        }
        Command::ModelRepair { profile, confirmed } => {
            let app_paths = paths()?;
            let runtime = current_runtime_asset().ok_or(CommandError::Usage(
                "no pinned llama.cpp runtime exists for this platform; use Remote mode",
            ))?;
            let model = dociler_core::assets::model_asset(profile);

            let cancellation = CancellationToken::new();
            let _signals = cli_signals::CliSignalGuard::install(cancellation.clone())
                .map_err(|_| CommandError::Signal)?;

            let model_state = inspect_cached_asset_cancellable(
                &app_paths,
                model.artifact(),
                VerificationLevel::Sha256,
                Some(&cancellation),
            );
            let runtime_archive_state = inspect_cached_asset_cancellable(
                &app_paths,
                runtime.artifact(),
                VerificationLevel::Sha256,
                Some(&cancellation),
            );
            let installed_runtime_state = inspect_installed_runtime_cancellable(
                &app_paths,
                *runtime,
                VerificationLevel::Sha256,
                Some(&cancellation),
            );

            let model_needs_repair = !model_state.state().is_verified();
            let runtime_archive_needs_repair = !runtime_archive_state.state().is_verified();
            let runtime_install_needs_repair =
                installed_runtime_state.state() != RuntimeInstallState::Verified;
            let needs_repair =
                model_needs_repair || runtime_archive_needs_repair || runtime_install_needs_repair;

            if !confirmed {
                writeln!(output, "Inspecting assets for {}:", profile.alias())?;
                writeln!(output, "  model-status={:?}", model_state.state())?;
                writeln!(
                    output,
                    "  runtime-archive-status={:?}",
                    runtime_archive_state.state()
                )?;
                writeln!(
                    output,
                    "  runtime-install-status={:?}",
                    installed_runtime_state.state()
                )?;
                if !needs_repair {
                    writeln!(
                        output,
                        "All assets for {} are intact and verified; no repair is needed.",
                        profile.alias()
                    )?;
                    return Ok(());
                }
                writeln!(
                    output,
                    "Repair required: invalid or missing files will be cleaned and re-downloaded/re-installed."
                )?;
                writeln!(
                    output,
                    "To execute repair, run: dociler model repair {} --confirm",
                    profile.alias()
                )?;
                return Err(CommandError::Usage(
                    "model repair requires explicit consent: dociler model repair PROFILE --confirm",
                ));
            }

            if !needs_repair {
                writeln!(
                    output,
                    "All assets for {} are already intact and verified; no repair was needed.",
                    profile.alias()
                )?;
                return Ok(());
            }

            print_asset_manifest_header(output)?;
            writeln!(output, "Repairing assets for {}:", profile.alias())?;

            if runtime_archive_needs_repair {
                writeln!(output, "Runtime archive requires repair; re-downloading:")?;
                let _ = remove_cached_assets(&app_paths, ModelRemoveTarget::Runtime, true);
                download_asset(
                    output,
                    &app_paths,
                    "llama.cpp runtime archive",
                    "MIT",
                    runtime.artifact(),
                    true,
                    &cancellation,
                )?;
            }

            if runtime_install_needs_repair {
                writeln!(output, "Installed runtime requires repair; re-installing:")?;
                let installed_dir = app_paths.runtimes_dir().join(LLAMA_CPP_RELEASE);
                if installed_dir.exists() {
                    let _ = fs::remove_dir_all(&installed_dir);
                }
                let _ = install_cached_runtime(
                    &app_paths,
                    *runtime,
                    RuntimeInstallOptions::confirmed(),
                    &cancellation,
                )
                .map_err(CommandError::RuntimeInstall)?;
                writeln!(output, "Runtime reinstalled and verified.")?;
            }

            if model_needs_repair {
                writeln!(
                    output,
                    "Model {} requires repair; re-downloading:",
                    profile.alias()
                )?;
                let _ = remove_cached_assets(&app_paths, ModelRemoveTarget::Profile(profile), true);
                download_asset(
                    output,
                    &app_paths,
                    profile.alias(),
                    model.license(),
                    model.artifact(),
                    true,
                    &cancellation,
                )?;
            }

            writeln!(
                output,
                "Repair completed successfully for {}.",
                profile.alias()
            )?;
        }
        Command::ModelRuntimeInstall { confirmed } => {
            if !confirmed {
                return Err(CommandError::Usage(
                    "runtime installation requires explicit consent: dociler model runtime-install --confirm",
                ));
            }
            let app_paths = paths()?;
            let runtime = current_runtime_asset().ok_or(CommandError::Usage(
                "no pinned llama.cpp runtime exists for this platform; use Remote mode",
            ))?;
            print_asset_manifest_header(output)?;
            writeln!(output, "Preparing the verified current-platform runtime:")?;
            writeln!(
                output,
                "  archive={:?}",
                runtime.artifact().cache_path(&app_paths)
            )?;
            writeln!(output, "  archive-sha256={}", runtime.artifact().sha256())?;
            writeln!(
                output,
                "  policy=private staging; bounded entries/bytes; no links or special files in the installed tree"
            )?;
            output.flush()?;
            let outcome = install_cached_runtime(
                &app_paths,
                *runtime,
                RuntimeInstallOptions::confirmed(),
                &CancellationToken::new(),
            )
            .map_err(CommandError::RuntimeInstall)?;
            let inspection =
                inspect_installed_runtime(&app_paths, *runtime, VerificationLevel::Sha256);
            match outcome {
                RuntimeInstallOutcome::AlreadyInstalled => {
                    writeln!(output, "Runtime already installed and verified.")?;
                }
                RuntimeInstallOutcome::Installed {
                    file_count,
                    expanded_bytes,
                } => {
                    writeln!(
                        output,
                        "Installed and inventoried {file_count} files ({expanded_bytes} bytes)."
                    )?;
                }
            }
            writeln!(output, "Runtime directory: {:?}", inspection.path())?;
            writeln!(output, "llama-server: {:?}", inspection.server_path())?;
            writeln!(
                output,
                "The runtime was not launched or probed; local inference remains disabled."
            )?;
        }
        Command::ModelRuntimeProbe { confirmed } => {
            if !confirmed {
                return Err(CommandError::Usage(
                    "runtime probe executes a local process and requires explicit consent: dociler model runtime-probe --confirm",
                ));
            }
            let app_paths = paths()?;
            let runtime = current_runtime_asset().ok_or(CommandError::Usage(
                "no pinned llama.cpp runtime exists for this platform; use Remote mode",
            ))?;
            let cancellation = CancellationToken::new();
            let _signals = cli_signals::CliSignalGuard::install(cancellation.clone())
                .map_err(|_| CommandError::Signal)?;
            writeln!(
                output,
                "Reverifying the pinned runtime before a model-free loopback probe. No GGUF will be loaded."
            )?;
            output.flush()?;
            let report = probe_installed_runtime(
                &app_paths,
                *runtime,
                RuntimeProbeOptions::confirmed(),
                &cancellation,
            )
            .map_err(CommandError::RuntimeProbe)?;
            writeln!(
                output,
                "Runtime probe passed: {} health/auth/version; startup and shutdown in {} ms.",
                report.build, report.startup_ms
            )?;
            writeln!(
                output,
                "Captured {} diagnostic bytes in memory (at most {} retained); no server remains running.",
                report.diagnostic_bytes_seen, report.diagnostic_bytes_retained
            )?;
            writeln!(
                output,
                "Model loading, local chat, and the Dociler API remain disabled."
            )?;
        }
        Command::ModelLoadProbe {
            profile,
            confirmed,
            allow_experimental,
            memory_ceiling_bytes,
        } => {
            if !confirmed {
                return Err(CommandError::Usage(
                    "model load probe executes a local model and requires explicit consent: dociler model load-probe PROFILE --confirm",
                ));
            }
            let app_paths = paths()?;
            let runtime = current_runtime_asset().ok_or(CommandError::Usage(
                "no pinned llama.cpp runtime exists for this platform; use Remote mode",
            ))?;
            let cancellation = CancellationToken::new();
            let _signals = cli_signals::CliSignalGuard::install(cancellation.clone())
                .map_err(|_| CommandError::Signal)?;
            writeln!(
                output,
                "Reverifying {} and the pinned runtime; a diagnostic 1024-token-context CPU process will briefly run.",
                profile.alias()
            )?;
            writeln!(
                output,
                "This does not qualify the full context, memory target, model quality, or local chat."
            )?;
            if allow_experimental {
                writeln!(
                    output,
                    "Experimental 6–8 GB hardware tier is explicitly permitted."
                )?;
            }
            if let Some(ceiling) = memory_ceiling_bytes {
                writeln!(
                    output,
                    "Enforcing process-group memory ceiling of {ceiling} bytes."
                )?;
            }
            output.flush()?;
            let options = ModelProbeOptions {
                confirmed: true,
                allow_experimental,
                memory_ceiling_bytes,
            };
            let report = probe_cached_model_with_options(
                &app_paths,
                *runtime,
                profile,
                options,
                &cancellation,
            )
            .map_err(CommandError::ModelProbe)?;
            writeln!(
                output,
                "Model load probe passed for {} at {} tokens; health was ready in {} ms and generation took {} ms.",
                report.profile.alias(),
                report.context_tokens,
                report.startup_ms,
                report.generation_ms
            )?;
            writeln!(
                output,
                "Captured {} diagnostic bytes in memory; the process stopped. Local chat remains disabled.",
                report.diagnostic_bytes_seen
            )?;
            match report.server_peak_rss_bytes {
                Some(bytes) => writeln!(
                    output,
                    "Diagnostic server-only peak RSS: {bytes} bytes (Linux VmHWM)."
                )?,
                None => writeln!(
                    output,
                    "Diagnostic server-only peak RSS: unavailable on this platform; no memory qualification was performed."
                )?,
            }
            match report.process_group_peak_rss_bytes {
                Some(bytes) => writeln!(
                    output,
                    "Diagnostic process-group peak RSS: {bytes} bytes (Dociler host + runtime sidecar concurrent peak)."
                )?,
                None => writeln!(
                    output,
                    "Diagnostic process-group peak RSS: unavailable on this platform."
                )?,
            }
        }
        Command::ConnectList => {
            let loaded = settings()?;
            if loaded.settings.remote_profiles().is_empty() {
                writeln!(output, "No remote profiles saved.")?;
            } else {
                for profile in loaded.settings.remote_profiles() {
                    writeln!(
                        output,
                        "{}  {}  model={}  credential={}",
                        profile.name(),
                        profile.base_url(),
                        profile.model(),
                        if profile.needs_credential() {
                            "OS store"
                        } else {
                            "none"
                        }
                    )?;
                }
            }
        }
        Command::ConnectVerify { url, model } => {
            let secret = environment_secret()?;
            let profile = RemoteProfile::new("verification", &url, &model, secret.is_some())
                .map_err(CommandError::Remote)?;
            let client = RemoteClient::connect(profile.clone(), secret.as_ref())
                .map_err(CommandError::Remote)?;
            client.verify().map_err(CommandError::Remote)?;
            writeln!(output, "Verified endpoint: {}", profile.base_url())?;
            writeln!(output, "Verified model: {}", profile.model())?;
            writeln!(output, "Nothing saved.")?;
        }
        Command::ConnectAdd { name, url, model } => {
            let app_paths = paths()?;
            let secret = environment_secret()?;
            let profile = install_remote_profile(
                &app_paths,
                &name,
                &url,
                &model,
                secret,
                &CancellationToken::new(),
            )
            .map_err(|error| match error {
                ProfileInstallError::Config(kind) => CommandError::Config(kind),
                ProfileInstallError::Credential(error) => CommandError::Credential(error),
                ProfileInstallError::Remote(error) => CommandError::Remote(error),
                ProfileInstallError::Cancelled => CommandError::Remote(RemoteError::Cancelled),
            })?;
            writeln!(output, "Saved remote profile '{}'.", profile.name())?;
            writeln!(output, "Endpoint: {}", profile.base_url())?;
            writeln!(output, "Model: {}", profile.model())?;
            writeln!(output, "Use: dociler run {}", profile.name())?;
        }
        Command::ConnectCheck { name } => {
            let profile = check_remote_profile(&paths()?, &name, &CancellationToken::new())
                .map_err(map_profile_mutation)?;
            writeln!(output, "Connection ready for '{}'.", profile.name())?;
            writeln!(output, "Endpoint: {}", profile.base_url())?;
            writeln!(output, "Model: {}", profile.model())?;
            writeln!(output, "Nothing changed.")?;
        }
        Command::ConnectEdit {
            name,
            url,
            model,
            confirmed_destination,
        } => {
            let outcome = edit_remote_profile(
                &paths()?,
                &name,
                &url,
                &model,
                confirmed_destination,
                &CancellationToken::new(),
            )
            .map_err(map_profile_mutation)?;
            writeln!(output, "Verified and updated profile '{}'.", name)?;
            writeln!(output, "Endpoint: {}", outcome.profile().base_url())?;
            writeln!(output, "Model: {}", outcome.profile().model())?;
            writeln!(output, "The credential policy was preserved.")?;
        }
        Command::ConnectRemove { name, confirmed } => {
            if !confirmed {
                return Err(CommandError::Usage(
                    "profile removal requires: dociler connect remove NAME --confirm",
                ));
            }
            let outcome = remove_remote_profile(&paths()?, &name, &CancellationToken::new())
                .map_err(map_profile_mutation)?;
            writeln!(output, "Removed remote profile '{name}'.")?;
            if outcome.credential_cleanup_warning().is_some() {
                writeln!(
                    output,
                    "Warning: the profile was removed, but its OS credential could not be deleted."
                )?;
            }
        }
        Command::ConnectKey {
            name,
            clear,
            confirmed,
        } => {
            if !confirmed {
                return Err(CommandError::Usage(
                    "clearing a key requires: dociler connect key-clear NAME --confirm",
                ));
            }
            let secret = if clear {
                None
            } else {
                Some(environment_secret()?.ok_or(CommandError::Usage(
                    "key rotation requires a non-empty DOCILER_API_KEY",
                ))?)
            };
            let outcome =
                rotate_remote_credential(&paths()?, &name, secret, &CancellationToken::new())
                    .map_err(map_profile_mutation)?;
            writeln!(
                output,
                "Verified and updated credential policy for '{}'.",
                outcome.profile().name()
            )?;
            if outcome.credential_cleanup_warning().is_some() {
                writeln!(
                    output,
                    "Warning: keyless mode was saved, but the old OS credential could not be deleted."
                )?;
            }
        }
        Command::Run { name } => {
            let profile = remote_profile(&name)?;
            let secret = secret_for(&profile)?;
            let client =
                RemoteClient::connect(profile, secret.as_ref()).map_err(CommandError::Remote)?;
            let mut bytes = Vec::new();
            io::stdin()
                .lock()
                .take(64 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| CommandError::Input)?;
            if bytes.is_empty() || bytes.len() > 64 * 1024 {
                return Err(CommandError::Input);
            }
            let prompt = String::from_utf8(bytes).map_err(|_| CommandError::Input)?;
            if prompt.trim().is_empty() {
                return Err(CommandError::Input);
            }
            let mut session = Session::new(workspace()?);
            session
                .push(Role::User, prompt)
                .map_err(|_| CommandError::Input)?;
            let answer = client
                .stream_chat(session.messages(), |text| {
                    write!(output, "{text}").map_err(|_| RemoteError::Output)?;
                    output.flush().map_err(|_| RemoteError::Output)
                })
                .map_err(CommandError::Remote)?;
            session
                .push(Role::Assistant, answer)
                .map_err(|_| CommandError::Remote(RemoteError::ResponseLimit))?;
            writeln!(output)?;
        }
        Command::Doctor => {
            let workspace = workspace()?;
            let report =
                Diagnostics::inspect(workspace.root()).map_err(|_| CommandError::Workspace)?;
            let loaded = settings()?;
            writeln!(
                output,
                "Dociler {} — read-only diagnostics",
                env!("CARGO_PKG_VERSION")
            )?;
            // Debug formatting escapes terminal controls in an untrusted path.
            writeln!(output, "Workspace: {:?}", report.workspace)?;
            writeln!(
                output,
                "Platform: {} ({})",
                report.operating_system, report.architecture
            )?;
            print_settings(output, &loaded, &workspace)?;
            writeln!(
                output,
                "Remote text chat: interactive 'dociler chat [PROFILE]' or one-shot 'dociler run PROFILE'"
            )?;
            writeln!(
                output,
                "Document parsing and local inference: not implemented"
            )?;
            let app_paths = paths()?;
            let hardware = HardwareInventory::inspect(&app_paths.models_dir());
            print_hardware(output, &hardware)?;
            writeln!(output, "API server: not implemented (no listening ports)")?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(command) = parse(&args) else {
        // Do not echo arbitrary arguments: they may contain credentials or controls.
        let _ = writeln!(
            io::stderr().lock(),
            "dociler: unsupported arguments; use 'dociler --help'."
        );
        return ExitCode::from(2);
    };

    let is_interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let result = match command {
        Command::Launch if is_interactive => interactive(None),
        Command::Launch => execute(Command::Help, &mut io::stdout().lock()),
        Command::Chat { name } if is_interactive => interactive(name.as_deref()),
        Command::Chat { .. } => Err(CommandError::Input),
        command => execute(command, &mut io::stdout().lock()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandError::Output(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
            ExitCode::SUCCESS
        }
        Err(error) => {
            let message = match error {
                CommandError::Config(io::ErrorKind::AlreadyExists) => {
                    "configuration already exists; use 'dociler config show'. Nothing overwritten."
                }
                CommandError::Config(io::ErrorKind::PermissionDenied) => {
                    "configuration access denied; check ownership and user-only permissions at 'dociler config paths'."
                }
                CommandError::Config(io::ErrorKind::InvalidData) => {
                    "invalid or unsupported configuration; check schema v1 fields at 'dociler config paths'. No defaults substituted."
                }
                CommandError::Config(_) => {
                    "configuration unavailable; check 'dociler config paths' and any absolute DOCILER_CONFIG_DIR override."
                }
                CommandError::Workspace => {
                    "workspace unavailable; run from an accessible directory."
                }
                CommandError::Credential(CredentialError::Unavailable) => {
                    "required credential is missing or the OS credential service is unavailable."
                }
                CommandError::Credential(_) => {
                    "OS credential service denied the operation; no plaintext fallback was used."
                }
                CommandError::Input => {
                    "invalid input; use 'dociler --help'. Prompts for 'run' must be non-empty UTF-8 on stdin and at most 64 KiB."
                }
                CommandError::Usage(message) => message,
                CommandError::Conflict => {
                    "profile changed in another process; nothing overwritten. Refresh and retry."
                }
                CommandError::AssetVerification => {
                    "one or more cached assets are missing or invalid; nothing was changed."
                }
                CommandError::AssetDownload(error) => match error {
                    DownloadError::ConsentRequired => {
                        "asset download requires explicit --confirm consent; nothing was changed."
                    }
                    DownloadError::UnsupportedSource => {
                        "asset source or redirect was rejected; only pinned HTTPS origins are allowed."
                    }
                    DownloadError::UnsafePath => {
                        "asset cache path is unsafe; symlinks and non-directory ancestors are rejected."
                    }
                    DownloadError::ExistingInvalid => {
                        "an invalid final asset already exists; it was not overwritten."
                    }
                    DownloadError::InvalidPartial => {
                        "the partial asset is unsafe or larger than the manifest; inspect it or retry with --restart."
                    }
                    DownloadError::Busy => {
                        "another Dociler process is downloading this asset; retry after it finishes."
                    }
                    DownloadError::Connection => {
                        "asset connection failed; the verified final path was not changed and partial bytes remain resumable."
                    }
                    DownloadError::Upstream => {
                        "asset server returned an error; the verified final path was not changed."
                    }
                    DownloadError::ResumeRejected => {
                        "asset server rejected resume; the partial was preserved. Retry with --restart to fetch from byte zero."
                    }
                    DownloadError::InvalidResponse => {
                        "asset server returned inconsistent range or length metadata; the final path was not changed."
                    }
                    DownloadError::SizeMismatch => {
                        "asset transfer ended at the wrong size; the partial was preserved for retry."
                    }
                    DownloadError::HashMismatch => {
                        "asset SHA-256 did not match the built-in manifest; nothing was published. Retry with --restart."
                    }
                    DownloadError::Cancelled => {
                        "asset download was cancelled; partial bytes remain resumable."
                    }
                    DownloadError::PublishConflict => {
                        "the final asset changed while publishing; nothing was overwritten."
                    }
                    DownloadError::Io(_) => {
                        "asset cache I/O failed; check cache ownership, permissions, and free space."
                    }
                },
                CommandError::AssetRemoval(error) => match error {
                    ModelRemovalError::ConsentRequired => {
                        "model removal requires explicit --confirm consent; nothing was changed."
                    }
                    ModelRemovalError::UnsafePath => {
                        "asset cache path is unsafe; symlinks and non-directory ancestors are rejected."
                    }
                    ModelRemovalError::Io(_) => {
                        "asset cache removal I/O failed; check cache ownership, permissions, and free space."
                    }
                },
                CommandError::RuntimeInstall(error) => match error {
                    RuntimeInstallError::ConsentRequired => {
                        "runtime installation requires explicit --confirm consent; nothing was changed."
                    }
                    RuntimeInstallError::ArchiveMissing => {
                        "the pinned runtime archive is missing; run 'dociler model download PROFILE --confirm' first."
                    }
                    RuntimeInstallError::ArchiveUnverified => {
                        "the cached runtime archive failed size or SHA-256 verification; nothing was extracted."
                    }
                    RuntimeInstallError::UnsupportedArchive => {
                        "the pinned runtime archive format is unsupported on this platform."
                    }
                    RuntimeInstallError::UnsafeArchive => {
                        "the runtime archive contains an unsafe path, link, duplicate, or special entry; nothing was published."
                    }
                    RuntimeInstallError::LimitsExceeded => {
                        "the runtime archive exceeds Dociler's entry, file, path, or expanded-size limit."
                    }
                    RuntimeInstallError::ExpectedServerMissing => {
                        "the archive does not contain the expected llama-server executable; nothing was published."
                    }
                    RuntimeInstallError::ExistingInvalid => {
                        "an invalid installed runtime already exists; it was not overwritten."
                    }
                    RuntimeInstallError::Busy => {
                        "another Dociler process is installing this runtime; retry after it finishes."
                    }
                    RuntimeInstallError::Cancelled => {
                        "runtime installation was cancelled; private staging was removed."
                    }
                    RuntimeInstallError::PublishConflict => {
                        "the runtime install path changed while publishing; nothing was overwritten."
                    }
                    RuntimeInstallError::Io(_) => {
                        "runtime installation I/O failed; check ownership, permissions, and free space."
                    }
                },
                CommandError::RuntimeProbe(error) => match error {
                    RuntimeProbeError::ConsentRequired => {
                        "runtime execution requires explicit --confirm consent."
                    }
                    RuntimeProbeError::InstallMissing => {
                        "the pinned runtime is not installed; run 'dociler model runtime-install --confirm' first."
                    }
                    RuntimeProbeError::InstallInvalid => {
                        "the installed runtime failed full inventory verification; nothing was executed."
                    }
                    RuntimeProbeError::UnsupportedCpuInstructions(_) => {
                        "the host CPU lacks required instructions for this runtime; run 'dociler model status' for details."
                    }
                    RuntimeProbeError::Cancelled => {
                        "runtime probe was cancelled and its child stopped."
                    }
                    RuntimeProbeError::Randomness => {
                        "could not generate a private internal API key; nothing was started."
                    }
                    RuntimeProbeError::Spawn => {
                        "could not start the verified runtime; check platform compatibility and local execution policy."
                    }
                    RuntimeProbeError::VersionMismatch => {
                        "the runtime did not report the pinned llama.cpp build; its child was stopped."
                    }
                    RuntimeProbeError::ProcessExited => {
                        "the runtime exited before the probe completed; no server remains running."
                    }
                    RuntimeProbeError::TimedOut => {
                        "the runtime probe timed out; its child was stopped."
                    }
                    RuntimeProbeError::InvalidResponse => {
                        "the runtime did not return the expected model-free loopback health response; its child was stopped."
                    }
                    RuntimeProbeError::AuthenticationFailed => {
                        "the runtime did not enforce its internal API key; its child was stopped."
                    }
                    RuntimeProbeError::StopFailed => {
                        "runtime shutdown could not be confirmed; inspect local processes before retrying."
                    }
                    RuntimeProbeError::Io(_) => {
                        "runtime probe I/O failed; inspect the private runtime installation and retry."
                    }
                },
                CommandError::ModelProbe(error) => match error {
                    ModelProbeError::ConsentRequired => {
                        "model execution requires explicit --confirm consent."
                    }
                    ModelProbeError::Cancelled => "model probe was cancelled and stopped.",
                    ModelProbeError::ModelMissing => {
                        "the pinned GGUF is missing; download the selected profile first."
                    }
                    ModelProbeError::ModelInvalid => {
                        "the pinned GGUF failed full size/SHA-256 verification; nothing was executed."
                    }
                    ModelProbeError::RuntimeMissing => {
                        "the pinned runtime is not installed; run 'dociler model runtime-install --confirm' first."
                    }
                    ModelProbeError::RuntimeInvalid => {
                        "the installed runtime failed full inventory verification; nothing was executed."
                    }
                    ModelProbeError::UnsupportedCpuInstructions(_) => {
                        "the host CPU lacks required instructions for this runtime; run 'dociler model status' for details."
                    }
                    ModelProbeError::HardwareNotReady(_) => {
                        "live hardware preflight did not admit this model; run 'dociler model status' and consider Remote mode. Experimental Lite is not enabled by this diagnostic."
                    }
                    ModelProbeError::Runtime(_) => {
                        "the pinned runtime failed the model probe; check platform compatibility and local processes before retrying."
                    }
                    ModelProbeError::ModelIdentity => {
                        "the runtime did not report the selected model alias/path/build; its child was stopped."
                    }
                    ModelProbeError::Generation => {
                        "the bounded local generation check failed; its child was stopped."
                    }
                    ModelProbeError::TimedOut => {
                        "model load or generation timed out; its child was stopped."
                    }
                    ModelProbeError::MemoryLimitExceeded { .. } => {
                        "process-group memory exceeded the ceiling; its child was stopped to protect the host."
                    }
                },
                CommandError::Signal => {
                    "could not install terminal cancellation handlers; no probe was started."
                }
                CommandError::Cancelled => "operation was cancelled.",
                CommandError::Remote(error) => match error {
                    RemoteError::InvalidProfile => "invalid remote profile name, URL, or model.",
                    RemoteError::UnsafeEndpoint => {
                        "unsafe endpoint: use HTTPS for public hosts; HTTP is limited to loopback/private addresses; URLs must be root or /v1 without credentials, query, or fragment."
                    }
                    RemoteError::Resolution => {
                        "endpoint DNS resolution failed or returned no safe addresses."
                    }
                    RemoteError::Connection => "remote connection failed or timed out.",
                    RemoteError::Authentication => {
                        "remote authentication failed; check the OS-stored key or DOCILER_API_KEY."
                    }
                    RemoteError::Upstream => {
                        "remote endpoint returned an error or redirect; redirects are disabled."
                    }
                    RemoteError::InvalidResponse => {
                        "remote endpoint returned an invalid OpenAI-compatible response."
                    }
                    RemoteError::ModelUnavailable => {
                        "requested model was not listed by the remote endpoint."
                    }
                    RemoteError::ResponseLimit => {
                        "remote response exceeded Dociler's safety limit."
                    }
                    RemoteError::Cancelled => "remote response was cancelled.",
                    RemoteError::Output => "output failed while streaming the remote response.",
                },
                CommandError::Discovery(ref error) => match error {
                    DiscoveryError::Io(_) => "failed to read workspace files or directories.",
                    DiscoveryError::TotalSizeLimitExceeded { .. } => {
                        "discovered workspace documents exceeded the total size limit (250 MiB)."
                    }
                    DiscoveryError::FileLimitExceeded { .. } => {
                        "discovered workspace documents exceeded the maximum file count limit (10,000 files)."
                    }
                    DiscoveryError::PathEscape { .. } => {
                        "a symlink attempted to escape the workspace boundary; operation aborted."
                    }
                },
                CommandError::Terminal => {
                    "interactive terminal setup or event handling failed; terminal state was restored."
                }
                CommandError::Output(_) => "output failed; check the output destination.",
            };
            let _ = writeln!(io::stderr().lock(), "dociler: {message}");
            ExitCode::FAILURE
        }
    }
}

fn format_cli_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}
