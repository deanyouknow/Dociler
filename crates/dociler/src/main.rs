use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use dociler_core::config::{ConfigSource, ConfigStore, LoadedSettings};
use dociler_core::diagnostics::Diagnostics;
use dociler_core::paths::AppPaths;
use dociler_core::workspace::{Workspace, WritePolicy};

const HELP: &str = "Dociler — local-first document assistant (development build)

Usage: dociler [COMMAND]

Commands:
  doctor        Show basic workspace and platform diagnostics
  config paths  Show OS config/model/runtime/cache locations (no writes)
  config show   Validate settings and show current-workspace policy (no writes)
  config init   Create safe default settings; never overwrite an existing file
  help          Show this help

Options:
  -h, --help    Show this help
  -V, --version Show the build version

This build contains configuration and session foundations. Chat, document reading, model loading,
and the API server are not available yet. See IMPLEMENTATION_PLAN.md.
";

enum Command {
    Help,
    Version,
    Doctor,
    ConfigPaths,
    ConfigShow,
    ConfigInit,
}

fn parse(args: &[OsString]) -> Option<Command> {
    match args {
        [] => Some(Command::Help),
        [arg] if arg == "help" || arg == "--help" || arg == "-h" => Some(Command::Help),
        [arg] if arg == "--version" || arg == "-V" => Some(Command::Version),
        [arg] if arg == "doctor" => Some(Command::Doctor),
        [command, flag]
            if (command == "doctor" || command == "config")
                && (flag == "--help" || flag == "-h") =>
        {
            Some(Command::Help)
        }
        [command, action] if command == "config" && action == "paths" => Some(Command::ConfigPaths),
        [command, action] if command == "config" && action == "show" => Some(Command::ConfigShow),
        [command, action] if command == "config" && action == "init" => Some(Command::ConfigInit),
        _ => None,
    }
}

enum CommandError {
    Output(io::Error),
    Workspace,
    Config(io::ErrorKind),
}

impl From<io::Error> for CommandError {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
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
        "Credential store: interface only; OS adapter not implemented"
    )
}

fn execute(command: Command, output: &mut impl Write) -> Result<(), CommandError> {
    match command {
        Command::Help => write!(output, "{HELP}")?,
        Command::Version => writeln!(output, "dociler {}", env!("CARGO_PKG_VERSION"))?,
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
        Command::Doctor => {
            let workspace = workspace()?;
            let report =
                Diagnostics::inspect(workspace.root()).map_err(|_| CommandError::Workspace)?;
            let loaded = settings()?;
            writeln!(
                output,
                "Dociler {} — basic diagnostics",
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
                "Chat, document parsing, and inference: not implemented"
            )?;
            writeln!(output, "Memory/GPU readiness: not assessed")?;
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

    match execute(command, &mut io::stdout().lock()) {
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
                CommandError::Output(_) => "output failed; check the output destination.",
            };
            let _ = writeln!(io::stderr().lock(), "dociler: {message}");
            ExitCode::FAILURE
        }
    }
}
