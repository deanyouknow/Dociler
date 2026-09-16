use std::ffi::OsString;
use std::io::{self, IsTerminal, Read, Write};
use std::process::ExitCode;

use dociler_core::chat::CancellationToken;
use dociler_core::config::{ConfigSource, ConfigStore, LoadedSettings};
use dociler_core::credentials::{CredentialError, CredentialStore, OsCredentialStore, Secret};
use dociler_core::diagnostics::Diagnostics;
use dociler_core::paths::AppPaths;
use dociler_core::profiles::{
    ProfileInstallError, ProfileMutationError, check_remote_profile, edit_remote_profile,
    install_remote_profile, remove_remote_profile, rotate_remote_credential,
};
use dociler_core::remote::{RemoteClient, RemoteError, RemoteProfile};
use dociler_core::session::{Role, Session};
use dociler_core::workspace::{Workspace, WritePolicy};

mod tui;

const HELP: &str = "Dociler — local-first document assistant (development build)

Usage: dociler [COMMAND]

Commands:
  chat [NAME]   Open interactive terminal chat; optionally choose a profile
  doctor        Show basic workspace and platform diagnostics
  config paths  Show OS config/model/runtime/cache locations (no writes)
  config show   Validate settings and show current-workspace policy (no writes)
  config init   Create safe default settings; never overwrite an existing file
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
Document reading, local model loading, and the Dociler API server are not available yet.
";

enum Command {
    Launch,
    Help,
    Version,
    Doctor,
    ConfigPaths,
    ConfigShow,
    ConfigInit,
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
}

fn parse(args: &[OsString]) -> Option<Command> {
    match args {
        [] => Some(Command::Launch),
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

enum CommandError {
    Output(io::Error),
    Workspace,
    Config(io::ErrorKind),
    Credential(CredentialError),
    Input,
    Usage(&'static str),
    Conflict,
    Remote(RemoteError),
    Terminal,
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
        "Credential store: native OS adapter (availability checked only when used)"
    )?;
    writeln!(
        output,
        "Saved remote profiles: {}",
        loaded.settings.remote_profiles().len()
    )
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
                "Remote text chat: interactive 'dociler chat [PROFILE]' or one-shot 'dociler run PROFILE'"
            )?;
            writeln!(
                output,
                "Document parsing and local inference: not implemented"
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
