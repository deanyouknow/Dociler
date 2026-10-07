use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use dociler_core::assets::{
    CacheState, ModelRemoveTarget, VerificationLevel, current_runtime_asset, inspect_cached_asset,
    model_asset, preview_cached_removal, remove_cached_assets,
};
use dociler_core::chat::{
    CancellationToken, ChatError, Generation, GenerationEvent, GenerationPoll, RemoteChatBackend,
};
use dociler_core::config::{ConfigStore, LocalProfile};
use dociler_core::credentials::{
    CredentialError, CredentialId, CredentialStore, OsCredentialStore, Secret,
};
use dociler_core::document::{
    Block as DocBlock, Document, DocumentFormat, DocumentMetadata, DocumentSource, InlineRun,
    SourceAnchor,
};
use dociler_core::editing::SessionUndoStack;
use dociler_core::export::{ExportFormat, export_document};
use dociler_core::hardware::{HardwareInventory, ModelPreflight, PreflightStatus};
use dociler_core::paths::AppPaths;
use dociler_core::profiles::{
    ProfileEdit, ProfileInstallError, ProfileMutationError, ProfileRemoval, ProfileRotation,
    check_remote_profile, edit_remote_profile, install_remote_profile, load_remote_profiles,
    remove_remote_profile, rotate_remote_credential,
};
use dociler_core::remote::{RemoteError, RemoteProfile};
use dociler_core::remote_serve::{
    DEFAULT_GATEWAY_PORT, GatewayConfig, GatewayHandle, ModelHealthStatus, generate_bearer_token,
    start_gateway,
};
use dociler_core::runtime_install::{RuntimeInstallState, inspect_installed_runtime};
use dociler_core::session::Session;
use dociler_core::workspace::{Workspace, WritePolicy};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use unicode_width::UnicodeWidthStr;
use zeroize::{Zeroize, Zeroizing};

const MAX_INPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
enum EntryKind {
    User,
    Assistant,
    Notice,
    Error,
}

struct TranscriptEntry {
    kind: EntryKind,
    text: Zeroizing<String>,
}

impl TranscriptEntry {
    fn new(kind: EntryKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: Zeroizing::new(text.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Ready,
    Generating,
    Cancelling,
    VerifyingProfile,
    CancellingProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnboardingStep {
    Name,
    Url,
    Model,
    ApiKey,
    ConfirmDestination,
    Verifying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnboardingKind {
    Add,
    RotateCredential,
    Edit,
}

struct Onboarding {
    kind: OnboardingKind,
    step: OnboardingStep,
    name: String,
    url: String,
    model: String,
    original_url: String,
    credential: bool,
}

impl Onboarding {
    fn add() -> Self {
        Self {
            kind: OnboardingKind::Add,
            step: OnboardingStep::Name,
            name: String::new(),
            url: String::new(),
            model: String::new(),
            original_url: String::new(),
            credential: false,
        }
    }

    fn rotate(profile: &RemoteProfile) -> Self {
        Self {
            kind: OnboardingKind::RotateCredential,
            step: OnboardingStep::ApiKey,
            name: profile.name().to_owned(),
            url: profile.base_url().to_owned(),
            model: profile.model().to_owned(),
            original_url: profile.base_url().to_owned(),
            credential: profile.needs_credential(),
        }
    }

    fn edit(profile: &RemoteProfile) -> Self {
        Self {
            kind: OnboardingKind::Edit,
            step: OnboardingStep::Url,
            name: profile.name().to_owned(),
            url: profile.base_url().to_owned(),
            model: profile.model().to_owned(),
            original_url: profile.base_url().to_owned(),
            credential: profile.needs_credential(),
        }
    }
}

enum ProfileWorkerEvent {
    Installed(Result<RemoteProfile, ProfileInstallError>),
    Removed {
        name: String,
        result: Result<ProfileRemoval, ProfileMutationError>,
    },
    Rotated(Result<ProfileRotation, ProfileMutationError>),
    Edited(Result<ProfileEdit, ProfileMutationError>),
    Checked {
        name: String,
        result: Result<RemoteProfile, ProfileMutationError>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConnectionStatus {
    Unknown,
    Checking(String),
    Ready(String),
    Failed(String),
}

struct PendingExport {
    format: ExportFormat,
    destination: PathBuf,
    document: Document,
    _source_name: String,
}

pub struct App {
    workspace: Workspace,
    paths: AppPaths,
    profiles: Vec<RemoteProfile>,
    selected: Option<usize>,
    session: Option<Session>,
    transcript: Vec<TranscriptEntry>,
    input: Zeroizing<String>,
    partial_answer: Zeroizing<String>,
    generation: Option<Generation>,
    generation_profile: Option<String>,
    onboarding: Option<Onboarding>,
    profile_worker: Option<Receiver<ProfileWorkerEvent>>,
    profile_cancel: Option<CancellationToken>,
    activity: Activity,
    scroll: u16,
    confirm_clear: bool,
    pending_remove: Option<String>,
    pending_model_remove: Option<ModelRemoveTarget>,
    pending_model_repair: Option<LocalProfile>,
    connection_status: ConnectionStatus,
    active_local: Option<LocalProfile>,
    undo_stack: SessionUndoStack,
    last_edited_path: Option<PathBuf>,
    pending_export: Option<PendingExport>,
    gateway_server: Option<GatewayHandle>,
    quit: bool,
}

impl App {
    pub fn new(
        workspace: Workspace,
        paths: AppPaths,
        profiles: Vec<RemoteProfile>,
        selected_name: Option<&str>,
    ) -> Self {
        let selected = selected_name
            .and_then(|name| profiles.iter().position(|profile| profile.name() == name))
            .or_else(|| (!profiles.is_empty()).then_some(0));
        let mut app = Self {
            session: Some(Session::new(workspace.clone())),
            workspace,
            paths,
            profiles,
            selected,
            transcript: Vec::new(),
            input: Zeroizing::new(String::new()),
            partial_answer: Zeroizing::new(String::new()),
            generation: None,
            generation_profile: None,
            onboarding: None,
            profile_worker: None,
            profile_cancel: None,
            activity: Activity::Ready,
            scroll: 0,
            confirm_clear: false,
            pending_remove: None,
            pending_model_remove: None,
            pending_model_repair: None,
            connection_status: ConnectionStatus::Unknown,
            active_local: None,
            undo_stack: SessionUndoStack::new(),
            last_edited_path: None,
            pending_export: None,
            gateway_server: None,
            quit: false,
        };
        app.notice("Welcome to Dociler. Text chat is memory-only; documents and local models are not active.");
        if app.selected.is_none() {
            app.notice("No remote profile is saved. Remote setup is open; local models arrive in a later milestone.");
            app.begin_onboarding();
        } else if app.profiles.len() > 1 {
            app.notice(
                "Multiple profiles found. Use /connect to list them or /connect NAME to switch.",
            );
        }
        app
    }

    #[cfg(test)]
    pub fn active_local_profile(&self) -> Option<LocalProfile> {
        self.active_local
    }

    fn selected_profile(&self) -> Option<&RemoteProfile> {
        self.selected.and_then(|index| self.profiles.get(index))
    }

    fn active_profile_is(&self, name: &str) -> bool {
        self.selected_profile()
            .is_some_and(|profile| profile.name() == name)
    }

    pub fn current_write_policy(&self) -> WritePolicy {
        let store = ConfigStore::new(self.paths.clone());
        match store.load() {
            Ok(loaded) => loaded.settings.write_policy(&self.workspace),
            Err(_) => WritePolicy::ReadOnly,
        }
    }

    pub fn shutdown_services(&mut self) {
        if let Some(server) = self.gateway_server.take() {
            server.shutdown();
        }
    }

    fn notice(&mut self, text: impl Into<String>) {
        self.transcript
            .push(TranscriptEntry::new(EntryKind::Notice, text));
        self.scroll = u16::MAX;
    }

    fn error(&mut self, text: impl Into<String>) {
        self.transcript
            .push(TranscriptEntry::new(EntryKind::Error, text));
        self.scroll = u16::MAX;
    }

    fn begin_onboarding(&mut self) {
        self.input.zeroize();
        self.input.clear();
        self.onboarding = Some(Onboarding::add());
        self.notice(
            "Remote setup: choose a short profile name. Nothing is saved until endpoint and model verification pass.",
        );
    }

    fn begin_credential_rotation(&mut self, profile: RemoteProfile) {
        self.input.zeroize();
        self.input.clear();
        let name = profile.name().to_owned();
        self.onboarding = Some(Onboarding::rotate(&profile));
        self.notice(format!(
            "Credential update for '{name}': enter a replacement API key, or press Enter to verify and switch to keyless access. Nothing changes unless verification passes."
        ));
    }

    fn begin_profile_edit(&mut self, profile: RemoteProfile) {
        self.input.zeroize();
        self.input.clear();
        self.input.push_str(profile.base_url());
        let name = profile.name().to_owned();
        self.onboarding = Some(Onboarding::edit(&profile));
        self.notice(format!(
            "Editing '{name}': confirm or replace the endpoint, then the model ID. The existing credential policy is preserved and the candidate must pass verification."
        ));
    }

    fn cancel_onboarding(&mut self) {
        self.input.zeroize();
        self.input.clear();
        let action = self.onboarding.as_ref().map(|onboarding| onboarding.kind);
        self.onboarding = None;
        self.notice(match action {
            Some(OnboardingKind::RotateCredential) => {
                "Credential update cancelled; the saved profile was not changed."
            }
            Some(OnboardingKind::Edit) => {
                "Profile edit cancelled; the saved endpoint and model were not changed."
            }
            _ => "Remote setup cancelled; nothing was saved.",
        });
    }

    fn submit(&mut self) {
        if self.activity != Activity::Ready {
            self.error("A response is already active. Press Esc to cancel it.");
            return;
        }
        let optional_key = self
            .onboarding
            .as_ref()
            .is_some_and(|onboarding| onboarding.step == OnboardingStep::ApiKey);
        let text = if optional_key {
            Zeroizing::new(self.input.to_string())
        } else {
            Zeroizing::new(self.input.trim().to_owned())
        };
        self.input.zeroize();
        self.input.clear();
        if text.is_empty() && !optional_key {
            return;
        }
        if self.onboarding.is_some() && text.as_str() == "/exit" {
            self.quit = true;
        } else if self.onboarding.is_some() && text.as_str() == "/help" {
            let help = match self.onboarding.as_ref().map(|onboarding| onboarding.kind) {
                Some(OnboardingKind::Edit) => {
                    "Profile edit fields: endpoint URL and model ID. The existing credential is reused only for verification. Use /back, /cancel or Escape, and /exit."
                }
                Some(OnboardingKind::RotateCredential) => {
                    "Credential update accepts a hidden replacement key; blank requests verified keyless access. Use /cancel or Escape to stop, and /exit to exit."
                }
                _ => {
                    "Remote setup fields: profile name, endpoint URL, model ID, then optional hidden API key. Use /back to edit the previous field, /cancel or Escape to stop, and /exit to exit Dociler."
                }
            };
            self.notice(help);
        } else if self.onboarding.is_some() && text.as_str() == "/back" {
            self.onboarding_back();
        } else if self.onboarding.is_some() && text.as_str() == "/cancel" {
            self.cancel_onboarding();
        } else if self.onboarding.is_some() {
            self.handle_onboarding_input(text.as_str());
        } else if text.starts_with('/') {
            self.handle_command(text.as_str());
        } else {
            self.pending_remove = None;
            self.pending_model_remove = None;
            self.pending_model_repair = None;
            self.start_generation(text.to_string());
        }
    }

    fn onboarding_back(&mut self) {
        let Some(onboarding) = &mut self.onboarding else {
            return;
        };
        if onboarding.kind == OnboardingKind::RotateCredential {
            self.notice(
                "This flow only changes the credential. Use /cancel to leave it unchanged.",
            );
            return;
        }
        if onboarding.kind == OnboardingKind::Edit {
            let message = match onboarding.step {
                OnboardingStep::Model => {
                    onboarding.step = OnboardingStep::Url;
                    self.input.push_str(&onboarding.url);
                    "Edit the endpoint URL."
                }
                OnboardingStep::ConfirmDestination => {
                    onboarding.step = OnboardingStep::Model;
                    self.input.push_str(&onboarding.model);
                    "Edit the upstream model ID."
                }
                OnboardingStep::Url => "Already at the first profile-edit field.",
                OnboardingStep::Verifying => {
                    "Verification is active. Press Escape to request cancellation first."
                }
                _ => "Profile edit only changes endpoint and model. Use /cancel to stop.",
            };
            self.notice(message);
            return;
        }
        let message = match onboarding.step {
            OnboardingStep::Name => "Already at the first setup field.",
            OnboardingStep::Url => {
                onboarding.step = OnboardingStep::Name;
                "Edit the profile name."
            }
            OnboardingStep::Model => {
                onboarding.step = OnboardingStep::Url;
                "Edit the endpoint URL."
            }
            OnboardingStep::ApiKey => {
                onboarding.step = OnboardingStep::Model;
                "Edit the upstream model ID."
            }
            OnboardingStep::ConfirmDestination => {
                onboarding.step = OnboardingStep::Model;
                "Edit the upstream model ID."
            }
            OnboardingStep::Verifying => {
                "Verification is active. Press Escape to request cancellation first."
            }
        };
        self.notice(message);
    }

    fn handle_onboarding_input(&mut self, input: &str) {
        let Some(step) = self.onboarding.as_ref().map(|onboarding| onboarding.step) else {
            return;
        };
        match step {
            OnboardingStep::Name => {
                if RemoteProfile::new(input, "https://example.invalid/v1", "model", false).is_err()
                {
                    self.error(
                        "Profile name must use 1–48 letters, numbers, hyphens, or underscores.",
                    );
                    return;
                }
                let onboarding = self.onboarding.as_mut().expect("onboarding remains active");
                onboarding.name = input.to_owned();
                onboarding.step = OnboardingStep::Url;
                self.notice("Enter the endpoint URL. Public hosts require HTTPS; private or loopback hosts may use HTTP.");
            }
            OnboardingStep::Url => {
                if RemoteProfile::new("validation", input, "model", false).is_err() {
                    self.error("Invalid endpoint. Use a root URL or /v1 with no credentials, query, or fragment.");
                    return;
                }
                let onboarding = self.onboarding.as_mut().expect("onboarding remains active");
                onboarding.url = input.to_owned();
                onboarding.step = OnboardingStep::Model;
                if onboarding.kind == OnboardingKind::Edit {
                    self.input.push_str(&onboarding.model);
                }
                self.notice("Enter the exact upstream model ID exposed by /v1/models.");
            }
            OnboardingStep::Model => {
                let onboarding = self.onboarding.as_ref().expect("onboarding remains active");
                if RemoteProfile::new(&onboarding.name, &onboarding.url, input, false).is_err() {
                    self.error(
                        "Invalid model ID. Use a non-empty printable value up to 256 bytes.",
                    );
                    return;
                }
                let kind = onboarding.kind;
                let destination_consent =
                    kind == OnboardingKind::Edit && onboarding.credential && {
                        let original = RemoteProfile::new(
                            &onboarding.name,
                            &onboarding.original_url,
                            input,
                            true,
                        )
                        .expect("saved profile remains valid");
                        let candidate =
                            RemoteProfile::new(&onboarding.name, &onboarding.url, input, true)
                                .expect("candidate was validated above");
                        !original.same_origin(&candidate)
                    };
                let onboarding = self.onboarding.as_mut().expect("onboarding remains active");
                onboarding.model = input.to_owned();
                if kind == OnboardingKind::Edit {
                    if destination_consent {
                        onboarding.step = OnboardingStep::ConfirmDestination;
                        let destination = onboarding.url.clone();
                        self.notice(format!(
                            "The saved API key will be sent to the new origin {} for verification. Type /confirm to allow this destination once, or use /back or /cancel.",
                            destination
                        ));
                    } else {
                        self.start_profile_edit(false);
                    }
                } else {
                    onboarding.step = OnboardingStep::ApiKey;
                    self.notice("Enter the API key, or press Enter for none. Key input is hidden and can only be saved in the OS credential store.");
                }
            }
            OnboardingStep::ApiKey => {
                if input.len() > 16 * 1024 || input.chars().any(char::is_control) {
                    self.error("API key must be at most 16 KiB with no control characters.");
                    return;
                }
                let secret = (!input.is_empty()).then(|| Secret::new(input.to_owned()));
                let kind = self
                    .onboarding
                    .as_ref()
                    .expect("onboarding remains active")
                    .kind;
                match kind {
                    OnboardingKind::Add => self.start_profile_install(secret),
                    OnboardingKind::RotateCredential => self.start_credential_rotation(secret),
                    OnboardingKind::Edit => unreachable!("profile edit has no API-key step"),
                }
            }
            OnboardingStep::ConfirmDestination => {
                if input == "/confirm" {
                    self.start_profile_edit(true);
                } else {
                    self.error("Type /confirm to send the saved key to the displayed new origin, or use /back or /cancel.");
                }
            }
            OnboardingStep::Verifying => {}
        }
    }

    fn start_profile_install(&mut self, secret: Option<Secret>) {
        let Some(onboarding) = &mut self.onboarding else {
            return;
        };
        onboarding.step = OnboardingStep::Verifying;
        let paths = self.paths.clone();
        let name = onboarding.name.clone();
        let url = onboarding.url.clone();
        let model = onboarding.model.clone();
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result =
                install_remote_profile(&paths, &name, &url, &model, secret, &worker_cancellation);
            let _ = sender.send(ProfileWorkerEvent::Installed(result));
        });
        self.profile_worker = Some(receiver);
        self.profile_cancel = Some(cancellation);
        self.activity = Activity::VerifyingProfile;
        self.notice("Verifying model listing and minimal generation… Escape requests cancellation before commit.");
    }

    fn start_profile_removal(&mut self, name: String) {
        let paths = self.paths.clone();
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker_name = name.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = remove_remote_profile(&paths, &worker_name, &worker_cancellation);
            let _ = sender.send(ProfileWorkerEvent::Removed {
                name: worker_name,
                result,
            });
        });
        self.profile_worker = Some(receiver);
        self.profile_cancel = Some(cancellation);
        self.activity = Activity::VerifyingProfile;
        self.notice(format!(
            "Removing profile '{name}' from configuration, then cleaning up its native credential…"
        ));
    }

    fn start_credential_rotation(&mut self, secret: Option<Secret>) {
        let Some(onboarding) = &mut self.onboarding else {
            return;
        };
        onboarding.step = OnboardingStep::Verifying;
        let paths = self.paths.clone();
        let name = onboarding.name.clone();
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = rotate_remote_credential(&paths, &name, secret, &worker_cancellation);
            let _ = sender.send(ProfileWorkerEvent::Rotated(result));
        });
        self.profile_worker = Some(receiver);
        self.profile_cancel = Some(cancellation);
        self.activity = Activity::VerifyingProfile;
        self.notice("Verifying the saved endpoint and model with the new credential policy… Escape requests cancellation before commit.");
    }

    fn start_profile_edit(&mut self, allow_credential_destination_change: bool) {
        let Some(onboarding) = &mut self.onboarding else {
            return;
        };
        onboarding.step = OnboardingStep::Verifying;
        let paths = self.paths.clone();
        let name = onboarding.name.clone();
        let url = onboarding.url.clone();
        let model = onboarding.model.clone();
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = edit_remote_profile(
                &paths,
                &name,
                &url,
                &model,
                allow_credential_destination_change,
                &worker_cancellation,
            );
            let _ = sender.send(ProfileWorkerEvent::Edited(result));
        });
        self.profile_worker = Some(receiver);
        self.profile_cancel = Some(cancellation);
        self.activity = Activity::VerifyingProfile;
        self.notice("Verifying the edited endpoint and model with the existing credential policy… Escape requests cancellation before commit.");
    }

    fn start_profile_check(&mut self, name: String) {
        let paths = self.paths.clone();
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker_name = name.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = check_remote_profile(&paths, &worker_name, &worker_cancellation);
            let _ = sender.send(ProfileWorkerEvent::Checked {
                name: worker_name,
                result,
            });
        });
        if self.active_profile_is(&name) {
            self.connection_status = ConnectionStatus::Checking(name.clone());
        }
        self.profile_worker = Some(receiver);
        self.profile_cancel = Some(cancellation);
        self.activity = Activity::VerifyingProfile;
        self.notice(format!(
            "Checking endpoint, credential, model listing, and minimal generation for '{name}'…"
        ));
    }

    fn replace_profiles(&mut self, profiles: Vec<RemoteProfile>, preferred: Option<&str>) -> bool {
        let previous = self.selected_profile().cloned();
        let preferred = preferred
            .map(str::to_owned)
            .or_else(|| previous.as_ref().map(|profile| profile.name().to_owned()));
        self.profiles = profiles;
        self.selected = preferred
            .as_deref()
            .and_then(|name| {
                self.profiles
                    .iter()
                    .position(|profile| profile.name() == name)
            })
            .or_else(|| (!self.profiles.is_empty()).then_some(0));
        let changed = self.selected_profile() != previous.as_ref();
        if changed {
            self.session = Some(Session::new(self.workspace.clone()));
            self.connection_status = ConnectionStatus::Unknown;
        }
        changed
    }

    fn refresh_profiles_from_disk(&mut self, announce: bool) -> Option<bool> {
        match load_remote_profiles(&self.paths) {
            Ok(profiles) => {
                let count = profiles.len();
                let changed = self.replace_profiles(profiles, None);
                if announce {
                    if changed {
                        self.notice(format!(
                            "Reloaded {count} saved profile(s). The active profile changed, so conversation context was reset."
                        ));
                    } else {
                        self.notice(format!(
                            "Reloaded {count} saved profile(s). Active conversation context was preserved."
                        ));
                    }
                } else if changed {
                    self.notice("Saved profile state changed in another process; active conversation context was reset before continuing.");
                }
                Some(changed)
            }
            Err(error) => {
                self.error(profile_mutation_error_message(error));
                None
            }
        }
    }

    fn connection_status_text(&self) -> String {
        let Some(profile) = self.selected_profile() else {
            return "unavailable — add or refresh a profile".to_owned();
        };
        match &self.connection_status {
            ConnectionStatus::Checking(name) if name == profile.name() => "checking now".to_owned(),
            ConnectionStatus::Ready(name) if name == profile.name() => {
                "ready — verified this session".to_owned()
            }
            ConnectionStatus::Failed(name) if name == profile.name() => format!(
                "needs attention — use /connect check {} or /connect edit {}",
                profile.name(),
                profile.name()
            ),
            _ => format!(
                "not checked this session — use /connect check {}",
                profile.name()
            ),
        }
    }

    fn connection_badge(&self) -> &'static str {
        let Some(profile) = self.selected_profile() else {
            return "offline";
        };
        match &self.connection_status {
            ConnectionStatus::Checking(name) if name == profile.name() => "checking",
            ConnectionStatus::Ready(name) if name == profile.name() => "ready",
            ConnectionStatus::Failed(name) if name == profile.name() => "attention",
            _ => "unchecked",
        }
    }

    fn reset_onboarding_for_retry(&mut self) {
        if let Some(onboarding) = &mut self.onboarding {
            onboarding.step = match onboarding.kind {
                OnboardingKind::Edit => OnboardingStep::Model,
                OnboardingKind::Add | OnboardingKind::RotateCredential => OnboardingStep::ApiKey,
            };
        }
    }

    fn handle_command(&mut self, input: &str) {
        let confirmed_clear = self.confirm_clear;
        self.confirm_clear = false;
        let pending_remove = self.pending_remove.take();
        let pending_model_remove = self.pending_model_remove.take();
        let pending_model_repair = self.pending_model_repair.take();
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or("");
        match command {
            "/help" => self.notice(
                "/help  /status  /files  /permissions [grant|revoke]  /export FORMAT DEST [SRC]  /undo [PATH]  /turn-on-remote  /turn-off-remote  /model [list|status|info|use|unload|remove|repair]  /connect [NAME|add|refresh|check|edit|remove|key]  /clear  /exit\nUse Tab for @file or slash-command completion. Exports and writes require confirmed permissions. Esc cancels active work; PageUp/PageDown scroll.",
            ),
            "/status" => {
                let write_policy = self.current_write_policy();
                let write_text = match write_policy {
                    WritePolicy::ReadOnly => "read-only (writes disabled)",
                    WritePolicy::ConfirmEveryWrite => "confirm-every-write (writes enabled)",
                };
                let api_text = if let Some(server) = &self.gateway_server {
                    if server.is_running() {
                        format!("on (http://{})", server.bound_addr())
                    } else {
                        "off".to_string()
                    }
                } else {
                    "off".to_string()
                };
                let status_text = if let Some(local) = self.active_local {
                    format!(
                        "Workspace: {}\nBackend: Local\nProfile: {}\nStatus: diagnostic only (local chat disabled pending release qualification)\nAccess: {}\nHistory: memory-only\nLAN API: {}",
                        safe_path(self.workspace.root()),
                        local.alias(),
                        write_text,
                        api_text
                    )
                } else {
                    let profile = self
                        .selected_profile()
                        .map(|profile| format!("{} ({})", profile.name(), profile.model()))
                        .unwrap_or_else(|| "none".to_owned());
                    format!(
                        "Workspace: {}\nBackend: Remote\nProfile: {profile}\nConnection: {}\nAccess: {}\nHistory: memory-only\nLAN API: {}",
                        safe_path(self.workspace.root()),
                        self.connection_status_text(),
                        write_text,
                        api_text
                    )
                };
                self.notice(status_text);
            }
            "/model" => {
                let arguments = parts.collect::<Vec<_>>();
                self.handle_model_command(&arguments, pending_model_remove, pending_model_repair);
            }
            "/connect" => {
                let arguments = parts.collect::<Vec<_>>();
                if arguments.as_slice() == ["refresh"] {
                    self.refresh_profiles_from_disk(true);
                    return;
                }
                if arguments.as_slice() != ["add"]
                    && self.refresh_profiles_from_disk(false).is_none()
                {
                    return;
                }
                match arguments.as_slice() {
                    [] if self.profiles.is_empty() => {
                        self.notice("No profiles saved. Use /connect add to open remote setup.");
                    }
                    [] => {
                        let profiles = self
                            .profiles
                            .iter()
                            .map(|profile| format!("{} ({})", profile.name(), profile.model()))
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.notice(format!("Saved profiles:\n{profiles}"));
                    }
                    ["add"] => self.begin_onboarding(),
                    ["check", name] => {
                        if self.profiles.iter().any(|profile| profile.name() == *name) {
                            self.start_profile_check((*name).to_owned());
                        } else {
                            self.error("Unknown profile. Use /connect refresh to reload saved profiles.");
                        }
                    }
                    ["edit", name] => {
                        if let Some(profile) = self
                            .profiles
                            .iter()
                            .find(|profile| profile.name() == *name)
                            .cloned()
                        {
                            self.begin_profile_edit(profile);
                        } else {
                            self.error("Unknown profile. Use /connect refresh to reload saved profiles.");
                        }
                    }
                    ["remove", name] => {
                        if !self.profiles.iter().any(|profile| profile.name() == *name) {
                            self.error("Unknown profile. Use /connect to list saved profiles.");
                        } else if pending_remove.as_deref() == Some(*name) {
                            self.start_profile_removal((*name).to_owned());
                        } else {
                            self.pending_remove = Some((*name).to_owned());
                            self.notice(format!(
                                "Repeat /connect remove {name} to remove the profile and its native credential."
                            ));
                        }
                    }
                    ["key", name] => {
                        if let Some(profile) = self
                            .profiles
                            .iter()
                            .find(|profile| profile.name() == *name)
                            .cloned()
                        {
                            self.begin_credential_rotation(profile);
                        } else {
                            self.error("Unknown profile. Use /connect to list saved profiles.");
                        }
                    }
                    [name] => {
                        if let Some(index) = self
                            .profiles
                            .iter()
                            .position(|profile| profile.name() == *name)
                        {
                            self.active_local = None;
                            self.selected = Some(index);
                            self.session = Some(Session::new(self.workspace.clone()));
                            self.connection_status = ConnectionStatus::Unknown;
                            self.confirm_clear = false;
                            self.notice(format!(
                                "Switched to {name}. Conversation context was reset."
                            ));
                        } else {
                            self.error("Unknown profile. Use /connect to list saved profiles.");
                        }
                    }
                    _ => self.error(
                        "Usage: /connect, /connect NAME, /connect add, /connect refresh, /connect check NAME, /connect edit NAME, /connect remove NAME, or /connect key NAME",
                    ),
                }
            }
            "/clear" if confirmed_clear => {
                if let Some(session) = &mut self.session {
                    session.clear();
                }
                self.transcript.clear();
                self.partial_answer.zeroize();
                self.partial_answer.clear();
                self.notice("Conversation memory cleared.");
            }
            "/clear" => {
                self.confirm_clear = true;
                self.notice("Repeat /clear to erase this in-memory conversation.");
            }
            "/exit" => self.quit = true,
            "/files" => match self.workspace.discover_documents() {
                Ok(report) => {
                    if report.documents.is_empty() {
                        self.notice(format!(
                            "No supported documents found in workspace ({}).",
                            self.workspace.root().display()
                        ));
                    } else {
                        let mut lines = Vec::new();
                        lines.push(format!(
                            "Workspace documents in {} ({} files, {}):",
                            self.workspace.root().display(),
                            report.documents.len(),
                            format_bytes(report.total_document_bytes)
                        ));
                        for doc in &report.documents {
                            lines.push(format!(
                                "  {} ({}, {}){}",
                                doc.relative_path.display(),
                                doc.format,
                                format_bytes(doc.byte_size),
                                if doc.is_direct_editable() {
                                    " [direct-editable]"
                                } else {
                                    ""
                                }
                            ));
                        }
                        self.notice(lines.join("\n"));
                    }
                }
                Err(err) => {
                    self.error(format!("Failed to discover documents: {err}"));
                }
            },
            "/permissions" => {
                let arguments = parts.collect::<Vec<_>>();
                match arguments.as_slice() {
                    [] => {
                        let policy = self.current_write_policy();
                        let canonical_path = safe_path(self.workspace.root());
                        let (state_label, action_hint) = match policy {
                            WritePolicy::ReadOnly => (
                                "disabled (read-only)",
                                "Use '/permissions grant' to enable write access.\n(The grant persists for this workspace in settings. Every write still requires a preview and explicit confirmation.)",
                            ),
                            WritePolicy::ConfirmEveryWrite => (
                                "enabled (confirm-every-write)",
                                "Use '/permissions revoke' to disable write access.\n(Every write operation requires a diff/format preview and explicit confirmation.)",
                            ),
                        };
                        self.notice(format!(
                            "Workspace write permissions:\n  Canonical path: {canonical_path}\n  Current policy: {state_label}\n\n{action_hint}"
                        ));
                    }
                    ["grant"] | ["enable"] => {
                        let store = ConfigStore::new(self.paths.clone());
                        match store.load() {
                            Ok(loaded) => {
                                let mut settings = loaded.settings;
                                match settings.grant_workspace_write(&self.workspace) {
                                    Ok(granted) => {
                                        if let Err(err) = store.save(&settings) {
                                            self.error(format!("Failed to save settings: {err}"));
                                        } else if granted {
                                            self.notice(format!(
                                                "Workspace write grant enabled for canonical workspace:\n  {}\nThe grant persists for this workspace in settings. Every write still requires a diff preview and explicit confirmation before any file is modified.",
                                                safe_path(self.workspace.root())
                                            ));
                                        } else {
                                            self.notice(format!(
                                                "Workspace write grant was already enabled for:\n  {}",
                                                safe_path(self.workspace.root())
                                            ));
                                        }
                                    }
                                    Err(_) => {
                                        self.error("Maximum number of workspace write grants (256) reached.");
                                    }
                                }
                            }
                            Err(err) => {
                                self.error(format!("Failed to load configuration: {err}"));
                            }
                        }
                    }
                    ["revoke"] | ["disable"] => {
                        let store = ConfigStore::new(self.paths.clone());
                        match store.load() {
                            Ok(loaded) => {
                                let mut settings = loaded.settings;
                                let revoked = settings.revoke_workspace_write(&self.workspace);
                                if let Err(err) = store.save(&settings) {
                                    self.error(format!("Failed to save settings: {err}"));
                                } else if revoked {
                                    self.notice(format!(
                                        "Workspace write grant revoked for:\n  {}\nWorkspace is now read-only.",
                                        safe_path(self.workspace.root())
                                    ));
                                } else {
                                    self.notice(format!(
                                        "Workspace was already read-only:\n  {}",
                                        safe_path(self.workspace.root())
                                    ));
                                }
                            }
                            Err(err) => {
                                self.error(format!("Failed to load configuration: {err}"));
                            }
                        }
                    }
                    _ => {
                        self.error("Usage: /permissions, /permissions grant, or /permissions revoke");
                    }
                }
            }
            "/turn-on-remote" => {
                if let Some(server) = &self.gateway_server {
                    if server.is_running() {
                        let token = server.current_token();
                        self.notice(format!(
                            "LAN gateway server is already running on http://{}.\nBearer token: {}\nUse /turn-off-remote to stop.",
                            server.bound_addr(),
                            token.as_str()
                        ));
                        return;
                    }
                }

                match generate_bearer_token() {
                    Ok(token) => {
                        let token_str = token.clone();
                        if let Ok(cred_id) = CredentialId::new("dociler-gateway") {
                            let _ = OsCredentialStore.set(&cred_id, &Secret::new(token_str.to_string()));
                        }

                        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], DEFAULT_GATEWAY_PORT));
                        let config = GatewayConfig {
                            bind_addr: addr,
                            active_model: self.active_local,
                            model_status: if self.active_local.is_some() {
                                ModelHealthStatus::Ready
                            } else {
                                ModelHealthStatus::Unloaded
                            },
                        };

                        match start_gateway(config, token) {
                            Ok(handle) => {
                                let bound = handle.bound_addr();
                                self.gateway_server = Some(handle);
                                self.notice(format!(
                                    "LAN gateway server started on http://{bound}\nBearer token: {}\nWarning: Dociler does not configure firewalls, routers, or internet TLS. Use only on a trusted private network.",
                                    token_str.as_str()
                                ));
                            }
                            Err(err) => {
                                self.error(format!(
                                    "Failed to start LAN gateway on port {DEFAULT_GATEWAY_PORT}: {err}. Ensure the port is not already in use."
                                ));
                            }
                        }
                    }
                    Err(err) => {
                        self.error(format!("Failed to generate secure bearer token: {err}"));
                    }
                }
            }
            "/turn-off-remote" => {
                if let Some(server) = self.gateway_server.take() {
                    server.shutdown();
                    self.notice("LAN gateway server stopped.");
                } else {
                    self.notice("LAN gateway server is not running.");
                }
            }
            "/export" => {
                let arguments = parts.collect::<Vec<_>>();
                self.handle_export_command(&arguments);
            }
            "/undo" => {
                let arguments = parts.collect::<Vec<_>>();
                self.handle_undo_command(&arguments);
            }
            "/update" => self.error(
                "Update checks and package installations are planned for a subsequent release.",
            ),
            _ => self.error("Unknown command. Use /help."),
        }
    }

    fn handle_model_command(
        &mut self,
        arguments: &[&str],
        pending_remove: Option<ModelRemoveTarget>,
        pending_repair: Option<LocalProfile>,
    ) {
        match arguments {
            [] | ["list"] => {
                let inventory = HardwareInventory::inspect(&self.paths.data_dir.join("models"));
                let mut lines = Vec::new();
                lines.push("Local model profiles:".to_string());
                for profile in [LocalProfile::Lite, LocalProfile::Pro] {
                    let asset = model_asset(profile);
                    let artifact = asset.artifact();
                    let inspection =
                        inspect_cached_asset(&self.paths, artifact, VerificationLevel::Sha256);
                    let cache_label = cache_state_label(inspection.state());
                    let preflight = ModelPreflight::evaluate(profile, &inventory);
                    let preflight_label = preflight_status_label(preflight.status());
                    let active_marker = if self.active_local == Some(profile) {
                        " [ACTIVE]"
                    } else {
                        ""
                    };
                    lines.push(format!(
                        "  {} ({}): {} · cache: {cache_label} · preflight: {preflight_label}{active_marker}",
                        profile.alias(),
                        asset.base_model(),
                        format_bytes(artifact.byte_size()),
                    ));
                }
                lines.push(String::new());
                lines.push("Commands:".to_string());
                lines.push("  /model status              - Show detailed hardware admission & cache details".to_string());
                lines.push(
                    "  /model info [PROFILE]      - Show profile and model metadata".to_string(),
                );
                lines.push("  /model use PROFILE         - Select a local profile (diagnostic only; local chat disabled)".to_string());
                lines.push(
                    "  /model unload              - Unselect active local profile".to_string(),
                );
                lines.push("  /model remove TARGET       - Safely remove cached assets (dry-run preview, repeat to confirm)".to_string());
                lines.push("  /model repair PROFILE      - Inspect and repair invalid assets (repeat to confirm)".to_string());
                self.notice(lines.join("\n"));
            }
            ["status"] => {
                let inventory = HardwareInventory::inspect(&self.paths.data_dir.join("models"));
                let total_ram = inventory
                    .total_memory_bytes()
                    .map(format_gib)
                    .unwrap_or_else(|| "unknown".to_string());
                let avail_ram = inventory
                    .available_memory_bytes()
                    .map(format_gib)
                    .unwrap_or_else(|| "unknown".to_string());
                let ram_scope = match inventory.memory_scope() {
                    dociler_core::hardware::MemoryScope::Host => "host",
                    dociler_core::hardware::MemoryScope::Cgroup => "cgroup",
                };
                let disk_space = inventory
                    .free_disk_bytes()
                    .map(format_gib)
                    .unwrap_or_else(|| "unknown".to_string());
                let cpu_features = if inventory.cpu_features().is_empty() {
                    "none detected".to_string()
                } else {
                    inventory.cpu_features().join(", ")
                };
                let lite_preflight = ModelPreflight::evaluate(LocalProfile::Lite, &inventory);
                let pro_preflight = ModelPreflight::evaluate(LocalProfile::Pro, &inventory);

                let lite_cache = inspect_cached_asset(
                    &self.paths,
                    model_asset(LocalProfile::Lite).artifact(),
                    VerificationLevel::Sha256,
                );
                let pro_cache = inspect_cached_asset(
                    &self.paths,
                    model_asset(LocalProfile::Pro).artifact(),
                    VerificationLevel::Sha256,
                );
                let runtime_cache = current_runtime_asset().map(|rt| {
                    inspect_cached_asset(&self.paths, rt.artifact(), VerificationLevel::Sha256)
                });
                let runtime_install = current_runtime_asset().map(|rt| {
                    inspect_installed_runtime(&self.paths, *rt, VerificationLevel::Sha256)
                });

                let mut lines = Vec::new();
                lines.push("Hardware & Admission Status:".to_string());
                lines.push(format!(
                    "  RAM: {total_ram} total, {avail_ram} available ({ram_scope})"
                ));
                lines.push(format!(
                    "  CPU: {} logical core(s), recommended threads: {}",
                    inventory.logical_cpu_count(),
                    inventory.recommended_threads()
                ));
                lines.push(format!("  CPU features: {cpu_features}"));
                if let Some(runtime) = current_runtime_asset() {
                    let cpu_check = inventory.evaluate_cpu_instructions(*runtime);
                    if cpu_check.supported {
                        lines.push(format!(
                            "  Runtime CPU: satisfied (required: {})",
                            if runtime.required_cpu_features().is_empty() {
                                "none".to_owned()
                            } else {
                                runtime.required_cpu_features().join(", ")
                            }
                        ));
                    } else {
                        lines.push(format!(
                            "  Runtime CPU: missing required ({})",
                            cpu_check.missing_required.join(", ")
                        ));
                    }
                }
                lines.push(format!("  Disk available: {disk_space}"));
                lines.push(format!(
                    "  Admission Lite: {}",
                    preflight_status_label(lite_preflight.status())
                ));
                lines.push(format!(
                    "  Admission Pro: {}",
                    preflight_status_label(pro_preflight.status())
                ));
                lines.push(String::new());
                lines.push("Local Cache Status:".to_string());
                lines.push(format!(
                    "  Lite GGUF: {}",
                    cache_state_label(lite_cache.state())
                ));
                lines.push(format!(
                    "  Pro GGUF: {}",
                    cache_state_label(pro_cache.state())
                ));
                lines.push(format!(
                    "  Runtime archive: {}",
                    runtime_cache
                        .map(|c| cache_state_label(c.state()))
                        .unwrap_or("unsupported target")
                ));
                lines.push(format!(
                    "  Runtime install: {}",
                    runtime_install
                        .map(|i| installed_state_label(i.state()))
                        .unwrap_or("unsupported target")
                ));
                self.notice(lines.join("\n"));
            }
            ["info"] => {
                let target = self.active_local.unwrap_or(LocalProfile::Lite);
                self.show_model_info(target);
            }
            ["info", name] => {
                let profile = match *name {
                    "dociler-lite" | "lite" => Some(LocalProfile::Lite),
                    "dociler-pro" | "pro" => Some(LocalProfile::Pro),
                    _ => None,
                };
                if let Some(profile) = profile {
                    self.show_model_info(profile);
                } else {
                    self.error("Unknown profile. Use 'dociler-lite' or 'dociler-pro'.");
                }
            }
            ["use", name] | [name]
                if matches!(*name, "dociler-lite" | "lite" | "dociler-pro" | "pro") =>
            {
                let profile = match *name {
                    "dociler-lite" | "lite" => LocalProfile::Lite,
                    "dociler-pro" | "pro" => LocalProfile::Pro,
                    _ => unreachable!(),
                };
                self.active_local = Some(profile);
                self.session = Some(Session::new(self.workspace.clone()));
                self.confirm_clear = false;
                self.notice(format!(
                    "Switched to local profile '{}'. Local chat is disabled pending release qualification (8K/16K context, peak RSS <= 5.5/11.5 GiB, quality gates). Use `dociler model load-probe {} --confirm` from the CLI for diagnostic checks, or /connect NAME to switch to a remote profile.",
                    profile.alias(),
                    profile.alias(),
                ));
            }
            ["use"] => {
                self.error("Usage: /model use [dociler-lite|dociler-pro]");
            }
            ["unload"] => {
                if self.active_local.is_some() {
                    self.active_local = None;
                    self.session = Some(Session::new(self.workspace.clone()));
                    self.confirm_clear = false;
                    self.notice("Local profile unloaded. Returned to remote backend.");
                } else {
                    self.notice("No local profile was active.");
                }
            }
            ["remove", target_name] => {
                let target = match *target_name {
                    "dociler-lite" | "lite" => Some(ModelRemoveTarget::Profile(LocalProfile::Lite)),
                    "dociler-pro" | "pro" => Some(ModelRemoveTarget::Profile(LocalProfile::Pro)),
                    "runtime" => Some(ModelRemoveTarget::Runtime),
                    "all" => Some(ModelRemoveTarget::All),
                    _ => None,
                };
                let Some(target) = target else {
                    self.error("Unknown remove target. Use 'dociler-lite', 'dociler-pro', 'runtime', or 'all'.");
                    return;
                };
                if pending_remove == Some(target) {
                    match remove_cached_assets(&self.paths, target, true) {
                        Ok(outcome) => {
                            let active_removed = match target {
                                ModelRemoveTarget::Profile(p) => self.active_local == Some(p),
                                ModelRemoveTarget::All => self.active_local.is_some(),
                                ModelRemoveTarget::Runtime => false,
                            };
                            if active_removed {
                                self.active_local = None;
                            }
                            self.notice(format!(
                                "Removed {} cached asset file(s) for target '{}', freeing {}.",
                                outcome.removed_paths.len(),
                                target.label(),
                                format_bytes(outcome.bytes_freed),
                            ));
                        }
                        Err(err) => {
                            self.error(format!("Removal failed: {err}"));
                        }
                    }
                } else {
                    match preview_cached_removal(&self.paths, target) {
                        Ok(preview) => {
                            if preview.removed_paths.is_empty() {
                                self.notice(format!(
                                    "No cached files found for '{}'.",
                                    target.label()
                                ));
                            } else {
                                self.pending_model_remove = Some(target);
                                self.notice(format!(
                                    "Repeat /model remove {} to permanently delete {} cached file(s) ({} recoverable).",
                                    target.label(),
                                    preview.removed_paths.len(),
                                    format_bytes(preview.bytes_freed),
                                ));
                            }
                        }
                        Err(err) => {
                            self.error(format!("Preview failed: {err}"));
                        }
                    }
                }
            }
            ["remove"] => {
                self.error("Usage: /model remove [dociler-lite|dociler-pro|runtime|all]");
            }
            ["repair", profile_name] => {
                let profile = match *profile_name {
                    "dociler-lite" | "lite" => Some(LocalProfile::Lite),
                    "dociler-pro" | "pro" => Some(LocalProfile::Pro),
                    _ => None,
                };
                let Some(profile) = profile else {
                    self.error("Unknown profile. Use 'dociler-lite' or 'dociler-pro'.");
                    return;
                };
                let model_inspection = inspect_cached_asset(
                    &self.paths,
                    model_asset(profile).artifact(),
                    VerificationLevel::Sha256,
                );
                let runtime_archive = current_runtime_asset().map(|rt| {
                    inspect_cached_asset(&self.paths, rt.artifact(), VerificationLevel::Sha256)
                });
                let runtime_install = current_runtime_asset().map(|rt| {
                    inspect_installed_runtime(&self.paths, *rt, VerificationLevel::Sha256)
                });

                let model_ok = model_inspection.state().is_verified();
                let runtime_archive_ok = runtime_archive
                    .as_ref()
                    .is_some_and(|a| a.state().is_verified());
                let runtime_install_ok = runtime_install
                    .as_ref()
                    .is_some_and(|i| i.state().is_verified());

                if model_ok && runtime_archive_ok && runtime_install_ok {
                    self.notice(format!(
                        "All assets for '{}' and the runtime are verified. No repair required.",
                        profile.alias(),
                    ));
                    return;
                }

                if pending_repair == Some(profile) {
                    if !model_ok && *model_inspection.state() != CacheState::Missing {
                        let _ = remove_cached_assets(
                            &self.paths,
                            ModelRemoveTarget::Profile(profile),
                            true,
                        );
                    }
                    let purge_runtime = (!runtime_archive_ok
                        && runtime_archive.as_ref().map(|a| a.state())
                            != Some(&CacheState::Missing))
                        || (!runtime_install_ok
                            && runtime_install.as_ref().map(|i| i.state())
                                != Some(RuntimeInstallState::Missing));
                    if purge_runtime {
                        let _ = remove_cached_assets(&self.paths, ModelRemoveTarget::Runtime, true);
                    }
                    self.notice(format!(
                        "Corrupted/invalid assets for '{}' were purged. Run `dociler model repair {} --confirm` or `dociler model download {} --confirm` from the CLI to complete network download and verification.",
                        profile.alias(),
                        profile.alias(),
                        profile.alias(),
                    ));
                } else {
                    self.pending_model_repair = Some(profile);
                    self.notice(format!(
                        "Repair required for '{}': model={}, runtime_archive={}, runtime_install={}.\nRepeat /model repair {} to purge invalid assets and prepare for re-download.",
                        profile.alias(),
                        if model_ok { "verified" } else { "needs repair" },
                        if runtime_archive_ok { "verified" } else { "needs repair" },
                        if runtime_install_ok { "verified" } else { "needs repair" },
                        profile.alias(),
                    ));
                }
            }
            ["repair"] => {
                self.error("Usage: /model repair [dociler-lite|dociler-pro]");
            }
            _ => {
                self.error(
                    "Usage: /model, /model status, /model info PROFILE, /model use PROFILE, /model unload, /model remove TARGET, or /model repair PROFILE",
                );
            }
        }
    }

    fn show_model_info(&mut self, profile: LocalProfile) {
        let asset = model_asset(profile);
        let artifact = asset.artifact();
        let reqs = dociler_core::hardware::ModelRequirements::for_profile(profile);
        let text = format!(
            "Profile: {}\nBase model: {}\nRepository: {}\nRevision: {}\nLicense: {}\nArtifact: {} ({})\nSHA-256: {}\nContext: {} tokens\nRAM required: {} (min {})\nDisk required: {}\nQualification: Local chat is disabled pending 8K/16K context, peak RSS <= 5.5/11.5 GiB, and quality gates.",
            profile.alias(),
            asset.base_model(),
            asset.repository(),
            asset.revision(),
            asset.license(),
            artifact.file_name(),
            format_bytes(artifact.byte_size()),
            artifact.sha256(),
            reqs.context_tokens(),
            format_gb(reqs.supported_total_memory_bytes()),
            format_gb(reqs.minimum_total_memory_bytes()),
            format_gb(reqs.required_free_disk_bytes()),
        );
        self.notice(text);
    }

    fn handle_export_command(&mut self, arguments: &[&str]) {
        match arguments {
            [] => {
                self.notice(
                    "Usage: /export FORMAT DEST_PATH [SOURCE_PATH]\nFormats: md, txt, docx, rtf, odt\nExamples:\n  /export docx export.docx\n  /export md notes.md input.docx\nExports require workspace write grant and preview confirmation via '/export confirm DEST_PATH'.",
                );
            }
            ["confirm", dest_str] => {
                let target = PathBuf::from(dest_str);
                let Some(pending) = self.pending_export.take() else {
                    self.error("No pending export to confirm. Use /export FORMAT DEST_PATH [SOURCE_PATH] to create a preview first.");
                    return;
                };

                if pending.destination != target {
                    self.error(format!(
                        "Destination mismatch: pending export is for '{}', not '{}'.",
                        pending.destination.display(),
                        target.display()
                    ));
                    self.pending_export = Some(pending);
                    return;
                }

                if self.current_write_policy() == WritePolicy::ReadOnly {
                    self.error("Workspace is read-only. Use '/permissions grant' to enable write access before exporting.");
                    return;
                }

                match export_document(
                    &self.workspace,
                    &pending.document,
                    pending.format,
                    &pending.destination,
                    false,
                ) {
                    Ok(report) => {
                        let mut msg = format!(
                            "Successfully exported to '{}' ({}, {}).",
                            report.destination_path.display(),
                            report.format.extension(),
                            format_bytes(report.byte_size as u64)
                        );
                        if !report.warnings.is_empty() {
                            msg.push_str("\nWarnings:\n");
                            for w in &report.warnings {
                                msg.push_str(&format!("  - {w}\n"));
                            }
                        }
                        self.notice(msg);
                    }
                    Err(err) => {
                        self.error(format!("Export failed: {err}"));
                    }
                }
            }
            [format_str, dest_str] | [format_str, dest_str, _] => {
                let format = match *format_str {
                    "md" | "markdown" => Some(ExportFormat::Markdown),
                    "txt" | "text" => Some(ExportFormat::PlainText),
                    "docx" => Some(ExportFormat::Docx),
                    "rtf" => Some(ExportFormat::Rtf),
                    "odt" => Some(ExportFormat::Odt),
                    _ => None,
                };

                let Some(export_fmt) = format else {
                    self.error(format!(
                        "Unknown export format '{format_str}'. Supported formats: md, txt, docx, rtf, odt"
                    ));
                    return;
                };

                let dest_path = PathBuf::from(*dest_str);
                let source_path_opt = arguments.get(2).map(|s| PathBuf::from(*s));

                let (doc, source_name) = if let Some(src_path) = source_path_opt {
                    let full_src = if src_path.is_relative() {
                        self.workspace.root().join(&src_path)
                    } else {
                        src_path.clone()
                    };

                    if !full_src.exists() {
                        self.error(format!(
                            "Source document '{}' does not exist.",
                            src_path.display()
                        ));
                        return;
                    }

                    let bytes = match std::fs::read(&full_src) {
                        Ok(b) => b,
                        Err(err) => {
                            self.error(format!(
                                "Failed to read source document '{}': {err}",
                                src_path.display()
                            ));
                            return;
                        }
                    };

                    let doc_fmt =
                        DocumentFormat::from_path(&full_src).unwrap_or(DocumentFormat::PlainText);
                    let parsed_doc = match doc_fmt {
                        DocumentFormat::Markdown => dociler_core::document::parse_markdown(
                            &src_path.to_string_lossy(),
                            &bytes,
                        )
                        .map_err(|e| e.to_string()),
                        DocumentFormat::PlainText => dociler_core::document::parse_plain_text(
                            &src_path.to_string_lossy(),
                            &bytes,
                        )
                        .map_err(|e| e.to_string()),
                        _ => {
                            let limits = dociler_core::extractor::ExtractionLimits::default();
                            let cancel = CancellationToken::new();
                            match dociler_core::extractor::extract_document_sandboxed(
                                &full_src,
                                &limits,
                                Some(&cancel),
                            ) {
                                Ok(doc) => Ok(doc),
                                Err(err) => {
                                    self.error(format!("Failed to extract source document: {err}"));
                                    return;
                                }
                            }
                        }
                    };

                    match parsed_doc {
                        Ok(d) => (d, src_path.to_string_lossy().to_string()),
                        Err(err) => {
                            self.error(format!("Failed to parse source document: {err}"));
                            return;
                        }
                    }
                } else {
                    if self.transcript.is_empty() {
                        self.error("No active conversation transcript to export. Specify a source document: /export FORMAT DEST SOURCE");
                        return;
                    }
                    (
                        self.document_from_transcript(),
                        "active conversation transcript".to_string(),
                    )
                };

                let block_count = doc.blocks.len();
                let dest_display = dest_path.display().to_string();
                self.pending_export = Some(PendingExport {
                    format: export_fmt,
                    destination: dest_path.clone(),
                    document: doc,
                    _source_name: source_name.clone(),
                });

                self.notice(format!(
                    "Export preview for '{dest_display}':\n  Format: {:?}\n  Source: {source_name}\n  Blocks: {block_count}\nRepeat '/export confirm {dest_display}' to write the exported document.",
                    export_fmt
                ));
            }
            _ => {
                self.error(
                    "Usage: /export FORMAT DEST_PATH [SOURCE_PATH] or /export confirm DEST_PATH",
                );
            }
        }
    }

    fn document_from_transcript(&self) -> Document {
        let mut blocks = Vec::new();
        let mut block_index = 0;
        blocks.push(DocBlock::Heading {
            level: 1,
            runs: vec![InlineRun::Text("Dociler Export".to_string())],
            anchor: SourceAnchor {
                page: None,
                section: Some("Dociler Export".to_string()),
                block_index,
                line_range: None,
            },
        });
        block_index += 1;

        for entry in &self.transcript {
            let (prefix, text) = match entry.kind {
                EntryKind::User => ("User: ", entry.text.as_str()),
                EntryKind::Assistant => ("Dociler: ", entry.text.as_str()),
                EntryKind::Notice => ("Notice: ", entry.text.as_str()),
                EntryKind::Error => ("Error: ", entry.text.as_str()),
            };
            for paragraph in text.split("\n\n") {
                let trimmed = paragraph.trim();
                if !trimmed.is_empty() {
                    blocks.push(DocBlock::Paragraph {
                        runs: vec![
                            InlineRun::Strong(prefix.to_string()),
                            InlineRun::Text(trimmed.to_string()),
                        ],
                        anchor: SourceAnchor {
                            page: None,
                            section: None,
                            block_index,
                            line_range: None,
                        },
                    });
                    block_index += 1;
                }
            }
        }

        let source = DocumentSource::from_bytes(
            "transcript_export",
            DocumentFormat::Markdown,
            b"Dociler Export",
        );
        Document::new(source, DocumentMetadata::default(), blocks)
    }

    fn handle_undo_command(&mut self, arguments: &[&str]) {
        let target = match arguments {
            [] => self.last_edited_path.clone(),
            [path_str] => Some(PathBuf::from(*path_str)),
            _ => {
                self.error("Usage: /undo [PATH]");
                return;
            }
        };

        let Some(target_path) = target else {
            self.notice("No edited file to undo in this session.");
            return;
        };

        if self.current_write_policy() == WritePolicy::ReadOnly {
            self.error("Workspace is read-only. Use '/permissions grant' to enable write access before undoing edits.");
            return;
        }

        match dociler_core::editing::undo_edit(&self.workspace, &target_path, &mut self.undo_stack)
        {
            Ok(()) => {
                self.notice(format!(
                    "Restored previous content for '{}' from session-memory undo stack.",
                    target_path.display()
                ));
            }
            Err(err) => {
                self.error(format!("Undo failed: {err}"));
            }
        }
    }

    fn handle_tab_completion(&mut self) {
        let current_input = self.input.as_str();
        if current_input.starts_with('/') && !current_input.contains(' ') {
            let available_commands = [
                "/help",
                "/status",
                "/files",
                "/permissions",
                "/export",
                "/undo",
                "/turn-on-remote",
                "/turn-off-remote",
                "/model",
                "/connect",
                "/clear",
                "/exit",
            ];
            let matches: Vec<&str> = available_commands
                .iter()
                .copied()
                .filter(|cmd| cmd.starts_with(current_input))
                .collect();
            match matches.as_slice() {
                [] => {
                    self.notice("No matching slash commands.");
                }
                [single] => {
                    self.input.zeroize();
                    self.input.clear();
                    self.input.push_str(single);
                    self.input.push(' ');
                }
                multiple => {
                    let common = common_prefix(multiple);
                    if common.len() > current_input.len() {
                        self.input.zeroize();
                        self.input.clear();
                        self.input.push_str(&common);
                    }
                    self.notice(format!("Available commands:\n  {}", multiple.join("  ")));
                }
            }
            return;
        }

        if let Some(at_idx) = current_input.rfind('@') {
            let query = &current_input[at_idx + 1..];
            if !query.contains(' ') && !query.contains('\n') {
                match self.workspace.discover_documents() {
                    Ok(report) => {
                        let candidate_files: Vec<String> = report
                            .documents
                            .iter()
                            .map(|d| d.relative_path.to_string_lossy().to_string())
                            .filter(|p| {
                                p.starts_with(query)
                                    || p.to_ascii_lowercase().contains(&query.to_ascii_lowercase())
                            })
                            .collect();

                        match candidate_files.as_slice() {
                            [] => {
                                self.notice(format!("No workspace documents matching '@{query}'."));
                            }
                            [single] => {
                                let prefix = &current_input[..at_idx];
                                let mut new_input =
                                    String::with_capacity(prefix.len() + 1 + single.len() + 1);
                                new_input.push_str(prefix);
                                new_input.push('@');
                                new_input.push_str(single);
                                new_input.push(' ');
                                self.input.zeroize();
                                self.input.clear();
                                self.input.push_str(&new_input);
                            }
                            multiple => {
                                let str_slice: Vec<&str> =
                                    multiple.iter().map(|s| s.as_str()).collect();
                                let common = common_prefix(&str_slice);
                                if common.len() > query.len() {
                                    let prefix = &current_input[..at_idx];
                                    let mut new_input =
                                        String::with_capacity(prefix.len() + 1 + common.len());
                                    new_input.push_str(prefix);
                                    new_input.push('@');
                                    new_input.push_str(&common);
                                    self.input.zeroize();
                                    self.input.clear();
                                    self.input.push_str(&new_input);
                                }
                                let preview_list = if multiple.len() > 8 {
                                    format!(
                                        "{} and {} more",
                                        multiple[..8].join(", "),
                                        multiple.len() - 8
                                    )
                                } else {
                                    multiple.join(", ")
                                };
                                self.notice(format!("Matching documents: {preview_list}"));
                            }
                        }
                    }
                    Err(err) => {
                        self.error(format!("Document discovery error: {err}"));
                    }
                }
                return;
            }
        }

        if current_input.is_empty() {
            self.notice(
                "Tab autocompletion: type '/' for slash commands or '@' for workspace documents.",
            );
        }
    }

    fn start_generation(&mut self, prompt: String) {
        if let Some(local) = self.active_local {
            let name = local.alias();
            self.error(format!(
                "Local chat for '{name}' is disabled pending release qualification (8K/16K context, peak RSS <= 5.5/11.5 GiB, quality gates). Use `dociler model load-probe {name} --confirm` from the CLI for diagnostic checks, or /connect NAME to switch to a remote profile."
            ));
            return;
        }
        match self.refresh_profiles_from_disk(false) {
            Some(false) => {}
            Some(true) => {
                self.error("The active profile changed before this prompt was sent. Review /status, then submit the prompt again to confirm the destination.");
                return;
            }
            None => return,
        }
        let Some(profile) = self.selected_profile().cloned() else {
            self.error("No active remote profile. Use /connect or add one outside this screen.");
            return;
        };
        let Some(session) = self.session.take() else {
            self.error("Conversation state is unavailable; use /clear before retrying.");
            return;
        };
        let profile_name = profile.name().to_owned();
        let generation = match Generation::start(
            session,
            prompt.clone(),
            Box::new(RemoteChatBackend::native(profile)),
        ) {
            Ok(generation) => generation,
            Err(session) => {
                self.session = Some(session);
                self.error("Conversation memory limit reached. Use /clear before continuing.");
                return;
            }
        };
        self.transcript
            .push(TranscriptEntry::new(EntryKind::User, prompt));
        self.partial_answer.zeroize();
        self.partial_answer.clear();
        self.confirm_clear = false;
        self.activity = Activity::Generating;
        self.generation_profile = Some(profile_name);
        self.scroll = u16::MAX;

        self.generation = Some(generation);
    }

    fn tick(&mut self) {
        self.tick_profile_worker();
        loop {
            let event = match self.generation.as_ref().map(Generation::poll) {
                Some((GenerationPoll::Event, Some(event))) => event,
                Some((GenerationPoll::Pending, None)) | None => break,
                Some((GenerationPoll::Disconnected, None)) => {
                    self.generation = None;
                    self.activity = Activity::Ready;
                    self.session = Some(Session::new(self.workspace.clone()));
                    if let Some(name) = self.generation_profile.take() {
                        self.connection_status = ConnectionStatus::Failed(name);
                    }
                    self.error(
                        "The response worker ended unexpectedly; conversation context was reset.",
                    );
                    break;
                }
                Some(_) => unreachable!("generation poll state and event are consistent"),
            };
            match event {
                GenerationEvent::Token(text) => {
                    self.partial_answer.push_str(text.as_str());
                    self.scroll = u16::MAX;
                }
                GenerationEvent::Finished(session) => {
                    self.finish_partial();
                    self.session = Some(session);
                    self.generation = None;
                    self.activity = Activity::Ready;
                    if let Some(name) = self.generation_profile.take() {
                        self.connection_status = ConnectionStatus::Ready(name);
                    }
                    break;
                }
                GenerationEvent::Failed(session, error) => {
                    self.finish_partial();
                    self.session = Some(session);
                    self.generation = None;
                    self.activity = Activity::Ready;
                    if connectivity_failure(error) {
                        if let Some(name) = self.generation_profile.take() {
                            self.connection_status = ConnectionStatus::Failed(name);
                        }
                    } else {
                        self.generation_profile = None;
                    }
                    self.error(worker_error_message(error));
                    break;
                }
            }
        }
    }

    fn tick_profile_worker(&mut self) {
        let event = match self.profile_worker.as_ref().map(Receiver::try_recv) {
            Some(Ok(event)) => event,
            Some(Err(TryRecvError::Empty)) | None => return,
            Some(Err(TryRecvError::Disconnected)) => {
                self.profile_worker = None;
                self.profile_cancel = None;
                self.activity = Activity::Ready;
                self.reset_onboarding_for_retry();
                let checking = match &self.connection_status {
                    ConnectionStatus::Checking(name) => Some(name.clone()),
                    _ => None,
                };
                if let Some(name) = checking {
                    self.connection_status = ConnectionStatus::Failed(name);
                }
                self.error("Profile worker ended unexpectedly; inspect saved configuration before retrying.");
                return;
            }
        };
        self.profile_worker = None;
        self.profile_cancel = None;
        self.activity = Activity::Ready;
        match event {
            ProfileWorkerEvent::Installed(result) => match result {
                Ok(profile) => {
                    let name = profile.name().to_owned();
                    match load_remote_profiles(&self.paths) {
                        Ok(profiles) => {
                            self.replace_profiles(profiles, Some(&name));
                        }
                        Err(_) => {
                            self.profiles.push(profile);
                            self.selected = Some(self.profiles.len() - 1);
                            self.session = Some(Session::new(self.workspace.clone()));
                        }
                    }
                    self.active_local = None;
                    self.connection_status = ConnectionStatus::Ready(name.clone());
                    self.onboarding = None;
                    self.notice(format!(
                        "Saved and selected profile '{}'. Conversation context is empty.",
                        name
                    ));
                }
                Err(ProfileInstallError::Cancelled) => {
                    self.onboarding = None;
                    self.notice("Remote setup cancelled; no profile or credential was saved.");
                }
                Err(error) => {
                    self.reset_onboarding_for_retry();
                    self.error(profile_install_error_message(error));
                }
            },
            ProfileWorkerEvent::Removed { name, result } => match result {
                Ok(outcome) => {
                    self.replace_profiles(outcome.profiles().to_vec(), None);
                    self.notice(format!("Removed profile '{name}'."));
                    if outcome.credential_cleanup_warning().is_some() {
                        self.error("The profile was removed, but its old native credential could not be deleted. Remove that credential manually from the OS credential manager.");
                    }
                    if self.profiles.is_empty() {
                        self.notice(
                            "No remote profile remains. Use /connect add to configure one.",
                        );
                    }
                }
                Err(ProfileMutationError::Cancelled) => {
                    self.notice("Profile removal cancelled before configuration changed.");
                }
                Err(error) => self.error(profile_mutation_error_message(error)),
            },
            ProfileWorkerEvent::Rotated(result) => match result {
                Ok(outcome) => {
                    let name = outcome.profile().name().to_owned();
                    self.replace_profiles(outcome.profiles().to_vec(), None);
                    if self.active_profile_is(&name) {
                        self.connection_status = ConnectionStatus::Ready(name.clone());
                    }
                    self.onboarding = None;
                    self.notice(format!(
                        "Verified and updated the credential policy for '{name}'."
                    ));
                    if outcome.credential_cleanup_warning().is_some() {
                        self.error("Keyless mode was saved, but the old native credential could not be deleted. Remove it manually from the OS credential manager.");
                    }
                }
                Err(ProfileMutationError::Cancelled) => {
                    self.onboarding = None;
                    self.notice("Credential update cancelled; the saved profile was not changed.");
                }
                Err(error) => {
                    self.reset_onboarding_for_retry();
                    self.error(profile_mutation_error_message(error));
                }
            },
            ProfileWorkerEvent::Edited(result) => match result {
                Ok(outcome) => {
                    let name = outcome.profile().name().to_owned();
                    self.replace_profiles(outcome.profiles().to_vec(), None);
                    if self.active_profile_is(&name) {
                        self.connection_status = ConnectionStatus::Ready(name.clone());
                    }
                    self.onboarding = None;
                    self.notice(format!(
                        "Verified and updated endpoint/model for '{name}'. Conversation context was reset if it was active."
                    ));
                }
                Err(ProfileMutationError::Cancelled) => {
                    self.onboarding = None;
                    self.notice("Profile edit cancelled; the saved profile was not changed.");
                }
                Err(error) => {
                    self.reset_onboarding_for_retry();
                    self.error(profile_mutation_error_message(error));
                }
            },
            ProfileWorkerEvent::Checked { name, result } => match result {
                Ok(checked) => {
                    let refreshed = self.refresh_profiles_from_disk(false);
                    if refreshed.is_some()
                        && self.profiles.iter().any(|profile| profile == &checked)
                    {
                        if self.active_profile_is(&name) {
                            self.connection_status = ConnectionStatus::Ready(name.clone());
                        }
                        self.notice(format!(
                            "Connection ready for '{name}': endpoint, credential, model listing, and minimal generation passed."
                        ));
                    } else if refreshed.is_some() {
                        self.connection_status = ConnectionStatus::Unknown;
                        self.notice("The profile changed while its connection check was running. Refreshed saved state; check it again before use.");
                    }
                }
                Err(ProfileMutationError::Cancelled) => {
                    if self.active_profile_is(&name) {
                        self.connection_status = ConnectionStatus::Unknown;
                    }
                    self.notice("Connection check cancelled; saved profile state was not changed.");
                }
                Err(error) => {
                    if self.active_profile_is(&name) {
                        self.connection_status = ConnectionStatus::Failed(name);
                    }
                    self.error(profile_mutation_error_message(error));
                }
            },
        }
    }

    fn finish_partial(&mut self) {
        if !self.partial_answer.is_empty() {
            let text = self.partial_answer.to_string();
            self.partial_answer.zeroize();
            self.partial_answer.clear();
            self.transcript
                .push(TranscriptEntry::new(EntryKind::Assistant, text));
        }
    }

    fn cancel(&mut self) {
        match self.activity {
            Activity::Generating | Activity::Cancelling => {
                if let Some(generation) = &self.generation {
                    generation.cancel();
                    self.activity = Activity::Cancelling;
                }
            }
            Activity::VerifyingProfile | Activity::CancellingProfile => {
                if let Some(cancellation) = &self.profile_cancel {
                    cancellation.cancel();
                    self.activity = Activity::CancellingProfile;
                }
            }
            Activity::Ready => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                if self.activity == Activity::Ready {
                    self.shutdown_services();
                    self.quit = true;
                } else {
                    self.cancel();
                }
            }
            (KeyCode::Esc, _) if self.activity != Activity::Ready => self.cancel(),
            (KeyCode::Esc, _) if self.onboarding.is_some() => self.cancel_onboarding(),
            (KeyCode::Esc, _) => {
                self.input.zeroize();
                self.input.clear();
            }
            (KeyCode::Enter, modifiers) if modifiers.contains(KeyModifiers::ALT) => {
                if self.input.len() < MAX_INPUT_BYTES {
                    self.input.push('\n');
                }
            }
            (KeyCode::Enter, _) => self.submit(),
            (KeyCode::Tab, _) if self.onboarding.is_none() => self.handle_tab_completion(),
            (KeyCode::Backspace, _) => {
                self.input.pop();
            }
            (KeyCode::Char(character), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
                if self.input.len() + character.len_utf8() <= MAX_INPUT_BYTES {
                    self.input.push(character);
                }
            }
            (KeyCode::PageUp, _) => self.scroll = self.scroll.saturating_sub(8),
            (KeyCode::PageDown, _) => self.scroll = self.scroll.saturating_add(8),
            _ => {}
        }
    }
}

fn connectivity_failure(error: ChatError) -> bool {
    matches!(error, ChatError::Credential(_))
        || matches!(
            error,
            ChatError::Remote(remote)
                if !matches!(remote, RemoteError::Cancelled | RemoteError::Output)
        )
}

fn worker_error_message(error: ChatError) -> &'static str {
    match error {
        ChatError::Credential(CredentialError::Unavailable) => {
            "Credential missing or OS credential service unavailable."
        }
        ChatError::Credential(_) => {
            "OS credential service denied access; no plaintext fallback was used."
        }
        ChatError::Remote(RemoteError::Cancelled) => {
            "Response cancelled. Partial text, if any, is not added to model context."
        }
        ChatError::Remote(RemoteError::UnsafeEndpoint) => {
            "Endpoint failed the HTTPS/private-address policy."
        }
        ChatError::Remote(RemoteError::Authentication) => "Remote authentication failed.",
        ChatError::Remote(RemoteError::ModelUnavailable) => {
            "The configured upstream model is unavailable."
        }
        ChatError::Remote(RemoteError::ResponseLimit) | ChatError::HistoryFull => {
            "Remote response exceeded a safety limit."
        }
        ChatError::Remote(RemoteError::Connection | RemoteError::Resolution) => {
            "Remote connection or DNS resolution failed."
        }
        ChatError::Remote(RemoteError::Upstream) => {
            "Remote endpoint returned an error or redirect."
        }
        ChatError::Remote(RemoteError::InvalidProfile | RemoteError::InvalidResponse) => {
            "Remote profile or response was invalid."
        }
        ChatError::Remote(RemoteError::Output) | ChatError::Delivery => {
            "Internal response delivery failed."
        }
    }
}

fn profile_install_error_message(error: ProfileInstallError) -> &'static str {
    match error {
        ProfileInstallError::Config(io::ErrorKind::AlreadyExists) => {
            "That profile name already exists. Setup was not saved; cancel and choose another name."
        }
        ProfileInstallError::Config(io::ErrorKind::PermissionDenied) => {
            "Configuration access was denied. Check user-only ownership and permissions."
        }
        ProfileInstallError::Config(_) => {
            "Configuration could not be safely updated; no plaintext credential fallback was used."
        }
        ProfileInstallError::Credential(CredentialError::Unavailable) => {
            "The OS credential service is unavailable. The API key was not saved."
        }
        ProfileInstallError::Credential(_) => {
            "The OS credential service denied access. The API key was not saved."
        }
        ProfileInstallError::Remote(RemoteError::UnsafeEndpoint) => {
            "Endpoint failed the HTTPS/private-address policy. API key input was discarded."
        }
        ProfileInstallError::Remote(RemoteError::Authentication) => {
            "Remote authentication failed. API key input was discarded; enter it again to retry."
        }
        ProfileInstallError::Remote(RemoteError::ModelUnavailable) => {
            "The model was not listed by the endpoint. Use /back to change the model ID."
        }
        ProfileInstallError::Remote(RemoteError::Connection | RemoteError::Resolution) => {
            "Remote connection or DNS resolution failed. API key input was discarded."
        }
        ProfileInstallError::Remote(RemoteError::Upstream) => {
            "Remote endpoint returned an error or redirect. API key input was discarded."
        }
        ProfileInstallError::Remote(RemoteError::InvalidProfile | RemoteError::InvalidResponse) => {
            "Remote profile or verification response was invalid. Nothing was saved."
        }
        ProfileInstallError::Remote(RemoteError::ResponseLimit) => {
            "Remote verification exceeded a safety limit. Nothing was saved."
        }
        ProfileInstallError::Remote(RemoteError::Cancelled) | ProfileInstallError::Cancelled => {
            "Remote setup was cancelled. Nothing was saved."
        }
        ProfileInstallError::Remote(RemoteError::Output) => {
            "Internal verification delivery failed. Nothing was saved."
        }
    }
}

fn profile_mutation_error_message(error: ProfileMutationError) -> &'static str {
    match error {
        ProfileMutationError::Config(io::ErrorKind::PermissionDenied) => {
            "Configuration access was denied; the requested profile change was not completed."
        }
        ProfileMutationError::Config(_) => {
            "Configuration could not be safely updated. Inspect the saved profile before retrying."
        }
        ProfileMutationError::Credential(CredentialError::Unavailable) => {
            "The OS credential service is unavailable; the credential change was not completed."
        }
        ProfileMutationError::Credential(_) => {
            "The OS credential service denied access; no plaintext fallback was used."
        }
        ProfileMutationError::CredentialRollback(_) => {
            "Configuration failed and native credential rollback also failed. Inspect the OS credential manager before retrying."
        }
        ProfileMutationError::Remote(RemoteError::Authentication) => {
            "Remote authentication failed. The saved credential was not changed; enter a new key to retry."
        }
        ProfileMutationError::Remote(RemoteError::ModelUnavailable) => {
            "The saved model is no longer listed by the endpoint. The profile was not changed."
        }
        ProfileMutationError::Remote(RemoteError::Connection | RemoteError::Resolution) => {
            "Remote connection or DNS resolution failed. The profile was not changed."
        }
        ProfileMutationError::Remote(RemoteError::UnsafeEndpoint) => {
            "The saved endpoint failed the HTTPS/private-address policy. The profile was not changed."
        }
        ProfileMutationError::Remote(_) => {
            "Remote verification failed. The profile was not changed."
        }
        ProfileMutationError::Cancelled => "The profile operation was cancelled.",
        ProfileMutationError::NotFound => {
            "The saved profile no longer exists. Use /connect to refresh the list."
        }
        ProfileMutationError::Conflict => {
            "The profile changed in another process while verification was running. Nothing was overwritten; use /connect refresh and retry."
        }
        ProfileMutationError::DestinationConsentRequired => {
            "The authenticated endpoint origin changed. No key was sent; review the displayed destination and explicitly confirm before retrying."
        }
    }
}

fn common_prefix(items: &[&str]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let first = items[0];
    let mut prefix_len = 0;
    for (i, c) in first.char_indices() {
        if items.iter().all(|s| s[i..].starts_with(c)) {
            prefix_len = i + c.len_utf8();
        } else {
            break;
        }
    }
    first[..prefix_len].to_string()
}

fn safe_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .chars()
        .flat_map(char::escape_default)
        .collect()
}

fn cache_state_label(state: &CacheState) -> &'static str {
    match state {
        CacheState::Missing => "missing",
        CacheState::PresentUnverified => "present; checksum not run",
        CacheState::Verified => "verified",
        CacheState::SizeMismatch { .. } => "size mismatch",
        CacheState::HashMismatch { .. } => "hash mismatch",
        CacheState::UnsafeFileType => "unsafe file type",
        CacheState::Unreadable(_) => "unreadable",
        CacheState::Cancelled => "cancelled",
    }
}

fn installed_state_label(state: RuntimeInstallState) -> &'static str {
    match state {
        RuntimeInstallState::Missing => "missing",
        RuntimeInstallState::PresentUnverified => "present; checksum not run",
        RuntimeInstallState::Verified => "verified",
        RuntimeInstallState::Invalid => "invalid",
        RuntimeInstallState::Cancelled => "cancelled",
    }
}

fn preflight_status_label(status: PreflightStatus) -> &'static str {
    match status {
        PreflightStatus::ReadyForRuntimeProbe => "ready for probe",
        PreflightStatus::ExperimentalForRuntimeProbe => "experimental (6–8 GB)",
        PreflightStatus::InsufficientTotalMemory => "insufficient RAM",
        PreflightStatus::InsufficientAvailableMemory => "low available RAM",
        PreflightStatus::InsufficientDisk => "insufficient disk",
        PreflightStatus::InventoryIncomplete => "hardware inventory incomplete",
    }
}

fn format_bytes(bytes: u64) -> String {
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

fn format_gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn format_gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let (backend_label, profile_label, connection_label) = if let Some(local) = app.active_local {
        (
            "Local",
            format!("{} · local chat disabled", local.alias()),
            "diagnostic only",
        )
    } else {
        (
            "Remote",
            app.selected_profile()
                .map(|profile| format!("{} · {}", profile.name(), profile.model()))
                .unwrap_or_else(|| "no remote profile".to_owned()),
            app.connection_badge(),
        )
    };
    let write_policy = app.current_write_policy();
    let write_label = match write_policy {
        WritePolicy::ReadOnly => "read-only",
        WritePolicy::ConfirmEveryWrite => "write-enabled",
    };
    let api_label = if let Some(server) = &app.gateway_server {
        if server.is_running() {
            format!("API on ({})", server.bound_addr())
        } else {
            "API off".to_string()
        }
    } else {
        "API off".to_string()
    };
    let header = Paragraph::new(vec![
        Line::from(Span::styled(
            "DOCILER",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "{backend_label} · {profile_label} · {connection_label} · {write_label} · memory-only · {api_label}"
        )),
    ])
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, areas[0]);

    let mut lines = Vec::new();
    for entry in &app.transcript {
        let (label, color) = match entry.kind {
            EntryKind::User => ("You", Color::Green),
            EntryKind::Assistant => ("Dociler", Color::Cyan),
            EntryKind::Notice => ("Info", Color::Yellow),
            EntryKind::Error => ("Error", Color::Red),
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(entry.text.as_str()));
        lines.push(Line::default());
    }
    if !app.partial_answer.is_empty() {
        lines.push(Line::from(Span::styled(
            "Dociler",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(app.partial_answer.as_str()));
    }
    let transcript_height = usize::from(areas[1].height.saturating_sub(2));
    let maximum_scroll = lines.len().saturating_sub(transcript_height) as u16;
    let scroll = if app.scroll == u16::MAX {
        maximum_scroll
    } else {
        app.scroll.min(maximum_scroll)
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Conversation "),
            )
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        areas[1],
    );

    let onboarding_step = app.onboarding.as_ref().map(|onboarding| onboarding.step);
    let masked_input = (onboarding_step == Some(OnboardingStep::ApiKey))
        .then(|| "•".repeat(app.input.chars().count()));
    let displayed_input = masked_input.as_deref().unwrap_or(app.input.as_str());
    let onboarding_kind = app.onboarding.as_ref().map(|onboarding| onboarding.kind);
    let composer_title = match (onboarding_kind, onboarding_step) {
        (Some(OnboardingKind::Edit), Some(OnboardingStep::Url)) => {
            " Profile edit 1/2 · Endpoint URL · Esc cancel "
        }
        (Some(OnboardingKind::Edit), Some(OnboardingStep::Model)) => {
            " Profile edit 2/2 · Model ID · Esc cancel "
        }
        (Some(OnboardingKind::Edit), Some(OnboardingStep::Verifying)) => {
            " Verifying profile edit · Esc cancel "
        }
        (Some(OnboardingKind::Edit), Some(OnboardingStep::ConfirmDestination)) => {
            " Confirm credential destination · type /confirm · Esc cancel "
        }
        (Some(OnboardingKind::RotateCredential), Some(OnboardingStep::ApiKey)) => {
            " Credential update · replacement key (hidden; blank removes) · Esc cancel "
        }
        (Some(OnboardingKind::RotateCredential), Some(OnboardingStep::Verifying)) => {
            " Verifying credential update · Esc cancel "
        }
        (_, Some(OnboardingStep::Name)) => " Remote setup 1/4 · Profile name · Esc cancel ",
        (_, Some(OnboardingStep::Url)) => " Remote setup 2/4 · Endpoint URL · Esc cancel ",
        (_, Some(OnboardingStep::Model)) => " Remote setup 3/4 · Model ID · Esc cancel ",
        (_, Some(OnboardingStep::ApiKey)) => {
            " Remote setup 4/4 · API key (hidden, optional) · Esc cancel "
        }
        (_, Some(OnboardingStep::ConfirmDestination)) => {
            " Confirm credential destination · type /confirm · Esc cancel "
        }
        (_, Some(OnboardingStep::Verifying)) => " Verifying remote profile · Esc cancel ",
        (_, None) => " Message · Enter send · Alt+Enter newline ",
    };
    let input_line_count = displayed_input.lines().count().max(1);
    let input_scroll = input_line_count.saturating_sub(2) as u16;
    frame.render_widget(
        Paragraph::new(displayed_input)
            .block(Block::default().borders(Borders::ALL).title(composer_title))
            .scroll((input_scroll, 0))
            .wrap(Wrap { trim: false }),
        areas[2],
    );
    let last_line = displayed_input.rsplit('\n').next().unwrap_or("");
    let cursor_width = UnicodeWidthStr::width(last_line) as u16;
    let cursor_x = areas[2]
        .x
        .saturating_add(1)
        .saturating_add(cursor_width)
        .min(areas[2].right().saturating_sub(2));
    let visible_line = (input_line_count.saturating_sub(1) as u16).saturating_sub(input_scroll);
    let cursor_y = areas[2]
        .y
        .saturating_add(1)
        .saturating_add(visible_line)
        .min(areas[2].bottom().saturating_sub(2));
    frame.set_cursor_position((cursor_x, cursor_y));
    let activity = match app.activity {
        Activity::Ready => "Ready",
        Activity::Generating => "Generating… Esc cancels",
        Activity::Cancelling => "Cancelling…",
        Activity::VerifyingProfile => "Verifying profile… Esc requests cancellation",
        Activity::CancellingProfile => "Cancelling profile setup…",
    };
    frame.render_widget(
        Paragraph::new(format!(
            " {activity}  |  {}  |  /help",
            safe_path(app.workspace.root())
        ))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        areas[3],
    );
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
    }
}

pub fn run(
    workspace: Workspace,
    paths: AppPaths,
    profiles: Vec<RemoteProfile>,
    selected_name: Option<&str>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(workspace, paths, profiles, selected_name);
    while !app.quit {
        app.tick();
        terminal.draw(|frame| render(frame, &app))?;
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Paste(text) => {
                    let remaining = MAX_INPUT_BYTES.saturating_sub(app.input.len());
                    let boundary = text
                        .char_indices()
                        .map(|(index, _)| index)
                        .take_while(|index| *index <= remaining)
                        .last()
                        .unwrap_or(0);
                    if remaining >= text.len() {
                        app.input.push_str(&text)
                    } else if boundary > 0 {
                        app.input.push_str(&text[..boundary])
                    }
                }
                _ => {}
            }
        }
    }
    app.shutdown_services();
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let paths = test_paths(&dir);
        let profiles = vec![
            RemoteProfile::new("one", "https://one.example", "model-1", false).unwrap(),
            RemoteProfile::new("two", "https://two.example", "model-2", false).unwrap(),
        ];
        let store = dociler_core::config::ConfigStore::new(paths.clone());
        let mut settings = store.load().unwrap().settings;
        for profile in &profiles {
            settings.add_remote_profile(profile.clone()).unwrap();
        }
        store.save(&settings).unwrap();
        (dir, App::new(workspace, paths, profiles, Some("one")))
    }

    fn test_paths(dir: &tempfile::TempDir) -> AppPaths {
        AppPaths::new(
            dir.path().join("config"),
            dir.path().join("data"),
            dir.path().join("cache"),
        )
        .unwrap()
    }

    fn command(app: &mut App, value: &str) {
        app.input.push_str(value);
        app.submit();
    }

    #[test]
    fn core_commands_are_deterministic_and_clear_requires_confirmation() {
        let (_dir, mut app) = app();
        command(&mut app, "/status");
        assert!(app.transcript.last().unwrap().text.contains("read-only"));
        command(&mut app, "/clear");
        assert!(app.confirm_clear);
        assert!(app.transcript.len() > 1);
        command(&mut app, "/clear");
        assert!(!app.confirm_clear);
        assert_eq!(app.transcript.len(), 1);
        command(&mut app, "/connect two");
        assert_eq!(app.selected_profile().unwrap().name(), "two");
        assert_eq!(app.session.as_ref().unwrap().messages().len(), 0);
        command(&mut app, "/exit");
        assert!(app.quit);
    }

    #[test]
    fn missing_profiles_and_planned_commands_fail_without_starting_work() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let paths = test_paths(&dir);
        let mut app = App::new(workspace, paths, Vec::new(), None);
        app.cancel_onboarding();
        command(&mut app, "hello");
        assert_eq!(app.activity, Activity::Ready);
        assert!(app.generation.is_none());
        command(&mut app, "/update");
        assert!(matches!(
            app.transcript.last().unwrap().kind,
            EntryKind::Error
        ));
    }

    #[test]
    fn files_command_lists_workspace_documents_or_shows_empty_notice() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let paths = test_paths(&dir);
        let mut app = App::new(workspace, paths, Vec::new(), None);
        app.cancel_onboarding();

        // Empty workspace
        command(&mut app, "/files");
        assert_eq!(app.activity, Activity::Ready);
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(
            last.text
                .contains("No supported documents found in workspace")
        );

        // With supported document
        std::fs::write(dir.path().join("overview.md"), "# Overview\nDociler").unwrap();
        command(&mut app, "/files");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(last.text.contains("overview.md"));
        assert!(last.text.contains("Markdown"));
        assert!(last.text.contains("[direct-editable]"));
    }

    #[test]
    fn render_exposes_state_without_terminal_io() {
        let (_dir, app) = app();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("DOCILER"));
        assert!(text.contains("one · model-1"));
        assert!(text.contains("read-only"));
        assert!(text.contains("memory-only"));
        assert!(text.contains("Message"));
    }

    #[test]
    fn input_is_bounded_and_escape_clears_it() {
        let (_dir, mut app) = app();
        for _ in 0..(MAX_INPUT_BYTES + 10) {
            app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        }
        assert_eq!(app.input.len(), MAX_INPUT_BYTES);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.input.is_empty());
    }

    #[test]
    fn escape_requests_cancellation_while_generation_is_active() {
        let (_dir, mut app) = app();
        struct WaitingBackend;

        impl dociler_core::chat::ChatBackend for WaitingBackend {
            fn stream(
                self: Box<Self>,
                _: &[dociler_core::session::Message],
                cancellation: &dociler_core::chat::CancellationToken,
                _: &mut dyn FnMut(&str) -> Result<(), ChatError>,
            ) -> Result<String, ChatError> {
                while !cancellation.is_cancelled() {
                    std::thread::yield_now();
                }
                Err(ChatError::Remote(RemoteError::Cancelled))
            }
        }

        let session = app.session.take().unwrap();
        app.generation =
            Generation::start(session, "hello".to_owned(), Box::new(WaitingBackend)).ok();
        app.activity = Activity::Generating;

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(app.generation.as_ref().unwrap().is_cancel_requested());
        assert_eq!(app.activity, Activity::Cancelling);
        assert!(!app.quit);
    }

    #[test]
    fn onboarding_validates_fields_and_masks_api_key_input() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let paths = test_paths(&dir);
        let mut app = App::new(workspace, paths, Vec::new(), None);
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Name);

        command(&mut app, "bad name");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Name);
        command(&mut app, "office");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Url);
        command(&mut app, "https://example.com/v1");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Model);
        command(&mut app, "upstream-model");
        assert_eq!(
            app.onboarding.as_ref().unwrap().step,
            OnboardingStep::ApiKey
        );
        app.input.push_str("do-not-render");

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains("do-not-render"));
        assert!(text.contains("••••"));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.onboarding.is_none());
        assert!(app.input.is_empty());
        assert!(!dir.path().join("config").exists());

        command(&mut app, "/connect add");
        command(&mut app, "/exit");
        assert!(app.quit);
        assert!(!dir.path().join("config").exists());
    }

    #[test]
    fn onboarding_supports_backtracking_and_explicit_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        let paths = test_paths(&dir);
        let mut app = App::new(workspace, paths, Vec::new(), None);

        command(&mut app, "office");
        command(&mut app, "https://example.com/v1");
        command(&mut app, "upstream-model");
        assert_eq!(
            app.onboarding.as_ref().unwrap().step,
            OnboardingStep::ApiKey
        );
        command(&mut app, "/back");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Model);
        command(&mut app, "/back");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Url);
        command(&mut app, "/cancel");

        assert!(app.onboarding.is_none());
        assert!(!dir.path().join("config").exists());
    }

    #[test]
    fn credential_update_opens_a_hidden_single_field_flow() {
        let (_dir, mut app) = app();
        command(&mut app, "/connect key one");

        let onboarding = app.onboarding.as_ref().unwrap();
        assert_eq!(onboarding.kind, OnboardingKind::RotateCredential);
        assert_eq!(onboarding.step, OnboardingStep::ApiKey);
        assert_eq!(onboarding.name, "one");
        app.input.push_str("replacement-secret");

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains("replacement-secret"));
        assert!(text.contains("Credential update"));
    }

    #[test]
    fn profile_removal_requires_exact_repetition_and_resets_active_context() {
        let (_dir, mut app) = app();
        let store = dociler_core::config::ConfigStore::new(app.paths.clone());
        app.session
            .as_mut()
            .unwrap()
            .push(
                dociler_core::session::Role::User,
                "private context".to_owned(),
            )
            .unwrap();

        command(&mut app, "/connect remove one");
        assert_eq!(app.pending_remove.as_deref(), Some("one"));
        assert_eq!(app.activity, Activity::Ready);
        command(&mut app, "/connect remove one");
        assert_eq!(app.activity, Activity::VerifyingProfile);

        for _ in 0..1_000 {
            app.tick();
            if app.activity == Activity::Ready {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        assert_eq!(app.activity, Activity::Ready);
        assert_eq!(app.profiles.len(), 1);
        assert_eq!(app.selected_profile().unwrap().name(), "two");
        assert!(app.session.as_ref().unwrap().messages().is_empty());
        assert!(
            store
                .load()
                .unwrap()
                .settings
                .remote_profile("one")
                .is_none()
        );
    }

    #[test]
    fn refresh_preserves_context_for_unrelated_changes_and_resets_changed_active_profile() {
        let (_dir, mut app) = app();
        let store = dociler_core::config::ConfigStore::new(app.paths.clone());
        app.session
            .as_mut()
            .unwrap()
            .push(
                dociler_core::session::Role::User,
                "keep until active changes".to_owned(),
            )
            .unwrap();

        let mut settings = store.load().unwrap().settings;
        settings
            .add_remote_profile(
                RemoteProfile::new("three", "https://three.example", "model-3", false).unwrap(),
            )
            .unwrap();
        store.save(&settings).unwrap();
        command(&mut app, "/connect refresh");
        assert_eq!(app.profiles.len(), 3);
        assert_eq!(app.session.as_ref().unwrap().messages().len(), 1);

        let mut settings = store.load().unwrap().settings;
        settings
            .replace_remote_profile(
                "one",
                RemoteProfile::new("one", "https://changed.example", "changed", false).unwrap(),
            )
            .unwrap();
        store.save(&settings).unwrap();
        command(&mut app, "/connect refresh");
        assert_eq!(app.selected_profile().unwrap().model(), "changed");
        assert!(app.session.as_ref().unwrap().messages().is_empty());
        assert_eq!(app.connection_status, ConnectionStatus::Unknown);
    }

    #[test]
    fn prompt_is_not_sent_after_a_concurrent_active_profile_change() {
        let (_dir, mut app) = app();
        let store = dociler_core::config::ConfigStore::new(app.paths.clone());
        let mut settings = store.load().unwrap().settings;
        settings
            .replace_remote_profile(
                "one",
                RemoteProfile::new(
                    "one",
                    "https://different-destination.example",
                    "different-model",
                    false,
                )
                .unwrap(),
            )
            .unwrap();
        store.save(&settings).unwrap();

        command(&mut app, "do not disclose this prompt");

        assert!(app.generation.is_none());
        assert_eq!(app.activity, Activity::Ready);
        assert!(app.session.as_ref().unwrap().messages().is_empty());
        assert!(app.transcript.iter().all(|entry| {
            entry.text.as_str() != "do not disclose this prompt"
                || matches!(entry.kind, EntryKind::Error | EntryKind::Notice)
        }));
        let error = app.transcript.last().unwrap().text.as_str();
        assert!(error.contains("active profile changed"));
        assert!(error.contains("submit the prompt again"));
    }

    #[test]
    fn profile_edit_prefills_fields_and_status_gives_a_recovery_action() {
        let (_dir, mut app) = app();
        command(&mut app, "/connect edit one");
        let onboarding = app.onboarding.as_ref().unwrap();
        assert_eq!(onboarding.kind, OnboardingKind::Edit);
        assert_eq!(onboarding.step, OnboardingStep::Url);
        assert_eq!(app.input.as_str(), "https://one.example/v1/");

        app.input.clear();
        command(&mut app, "https://replacement.example/v1");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Model);
        assert_eq!(app.input.as_str(), "model-1");
        app.input.clear();
        command(&mut app, "/back");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Url);
        assert_eq!(app.input.as_str(), "https://replacement.example/v1");
        app.cancel_onboarding();

        app.connection_status = ConnectionStatus::Failed("one".to_owned());
        command(&mut app, "/status");
        let status = app.transcript.last().unwrap().text.as_str();
        assert!(status.contains("needs attention"));
        assert!(status.contains("/connect check one"));
        assert!(status.contains("/connect edit one"));
    }

    #[test]
    fn authenticated_origin_edit_requires_explicit_destination_confirmation() {
        let (_dir, mut app) = app();
        let store = dociler_core::config::ConfigStore::new(app.paths.clone());
        let mut settings = store.load().unwrap().settings;
        settings
            .replace_remote_profile(
                "one",
                RemoteProfile::new("one", "https://one.example", "model-1", true).unwrap(),
            )
            .unwrap();
        store.save(&settings).unwrap();

        command(&mut app, "/connect edit one");
        app.input.clear();
        command(&mut app, "https://new-origin.example/v1");
        app.input.clear();
        command(&mut app, "new-model");

        assert_eq!(
            app.onboarding.as_ref().unwrap().step,
            OnboardingStep::ConfirmDestination
        );
        assert!(app.profile_worker.is_none());
        assert!(
            app.transcript
                .last()
                .unwrap()
                .text
                .contains("new-origin.example")
        );
        command(&mut app, "/back");
        assert_eq!(app.onboarding.as_ref().unwrap().step, OnboardingStep::Model);
    }

    #[test]
    fn model_list_status_and_info_commands_in_tui() {
        let (_dir, mut app) = app();
        command(&mut app, "/model");
        let list = app.transcript.last().unwrap().text.as_str();
        assert!(list.contains("Local model profiles:"));
        assert!(list.contains("dociler-lite"));
        assert!(list.contains("dociler-pro"));
        assert!(list.contains("/model status"));

        command(&mut app, "/model status");
        let status = app.transcript.last().unwrap().text.as_str();
        assert!(status.contains("Hardware & Admission Status:"));
        assert!(status.contains("RAM:"));
        assert!(status.contains("CPU:"));
        assert!(status.contains("Runtime CPU:"));
        assert!(status.contains("Disk available:"));
        assert!(status.contains("Admission Lite:"));
        assert!(status.contains("Admission Pro:"));
        assert!(status.contains("Local Cache Status:"));

        command(&mut app, "/model info dociler-lite");
        let info = app.transcript.last().unwrap().text.as_str();
        assert!(info.contains("dociler-lite"));
        assert!(info.contains("Qwen3.5 4B"));
        assert!(info.contains("Apache-2.0"));
        assert!(info.contains("8192 tokens"));
        assert!(info.contains("Local chat is disabled"));

        command(&mut app, "/model info unknown");
        let error = app.transcript.last().unwrap().text.as_str();
        assert!(error.contains("Unknown profile"));
    }

    #[test]
    fn model_use_and_prompting_disabled_in_tui() {
        let (_dir, mut app) = app();
        assert_eq!(app.active_local_profile(), None);

        command(&mut app, "/model use dociler-lite");
        assert_eq!(app.active_local_profile(), Some(LocalProfile::Lite));
        let switch_msg = app.transcript.last().unwrap().text.as_str();
        assert!(switch_msg.contains("Switched to local profile 'dociler-lite'"));
        assert!(switch_msg.contains("Local chat is disabled"));

        command(&mut app, "/status");
        let status = app.transcript.last().unwrap().text.as_str();
        assert!(status.contains("Backend: Local"));
        assert!(status.contains("Profile: dociler-lite"));
        assert!(status.contains("diagnostic only"));

        // Prompting while local profile is active must fail without starting generation
        command(&mut app, "Hello local model");
        assert!(app.generation.is_none());
        assert_eq!(app.activity, Activity::Ready);
        let prompt_err = app.transcript.last().unwrap().text.as_str();
        assert!(prompt_err.contains("Local chat for 'dociler-lite' is disabled"));

        // Switching back to remote via /connect
        command(&mut app, "/connect two");
        assert_eq!(app.active_local_profile(), None);
        assert_eq!(app.selected_profile().unwrap().name(), "two");

        // Selecting pro and unloading
        command(&mut app, "/model use dociler-pro");
        assert_eq!(app.active_local_profile(), Some(LocalProfile::Pro));
        command(&mut app, "/model unload");
        assert_eq!(app.active_local_profile(), None);
        let unload_msg = app.transcript.last().unwrap().text.as_str();
        assert!(unload_msg.contains("Local profile unloaded"));
    }

    #[test]
    fn model_remove_and_repair_in_tui_require_confirmation() {
        let (_dir, mut app) = app();

        // Model remove when empty reports no files found
        command(&mut app, "/model remove dociler-lite");
        let empty_preview = app.transcript.last().unwrap().text.as_str();
        assert!(empty_preview.contains("No cached files found"));

        // Stage a cached model file
        let asset = model_asset(LocalProfile::Lite);
        let path = asset.artifact().cache_path(&app.paths);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"test-model-content").unwrap();
        assert!(path.exists());

        // First remove command previews and requests repetition
        command(&mut app, "/model remove dociler-lite");
        assert_eq!(
            app.pending_model_remove,
            Some(ModelRemoveTarget::Profile(LocalProfile::Lite))
        );
        let preview_msg = app.transcript.last().unwrap().text.as_str();
        assert!(preview_msg.contains("Repeat /model remove dociler-lite"));

        // Exact repetition deletes the file
        command(&mut app, "/model remove dociler-lite");
        assert_eq!(app.pending_model_remove, None);
        assert!(!path.exists());
        let remove_msg = app.transcript.last().unwrap().text.as_str();
        assert!(remove_msg.contains("Removed 1 cached asset file(s)"));

        // Model repair requests confirmation when missing
        command(&mut app, "/model repair dociler-lite");
        assert_eq!(app.pending_model_repair, Some(LocalProfile::Lite));
        let repair_msg = app.transcript.last().unwrap().text.as_str();
        assert!(repair_msg.contains("Repair required for 'dociler-lite'"));
        assert!(repair_msg.contains("Repeat /model repair dociler-lite"));

        // Repetition confirms repair
        command(&mut app, "/model repair dociler-lite");
        assert_eq!(app.pending_model_repair, None);
        let repair_done = app.transcript.last().unwrap().text.as_str();
        assert!(repair_done.contains("purged"));
    }

    #[test]
    fn test_permissions_command_inspect_grant_revoke() {
        let (dir, mut app) = app();
        let _ = dir;

        // Default: read-only
        command(&mut app, "/permissions");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(last.text.contains("disabled (read-only)"));

        // Grant
        command(&mut app, "/permissions grant");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(last.text.contains("grant enabled"));
        assert_eq!(app.current_write_policy(), WritePolicy::ConfirmEveryWrite);

        // Revoke
        command(&mut app, "/permissions revoke");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(last.text.contains("grant revoked"));
        assert_eq!(app.current_write_policy(), WritePolicy::ReadOnly);
    }

    #[test]
    fn test_turn_on_and_off_remote_gateway_in_tui() {
        let (_dir, mut app) = app();

        // Turn on
        command(&mut app, "/turn-on-remote");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(last.text.contains("LAN gateway server started"));
        assert!(last.text.contains("Bearer token:"));
        assert!(app.gateway_server.is_some());

        // Already running notification
        command(&mut app, "/turn-on-remote");
        let last = app.transcript.last().unwrap();
        assert!(last.text.contains("already running"));

        // Turn off
        command(&mut app, "/turn-off-remote");
        let last = app.transcript.last().unwrap();
        assert!(last.text.contains("server stopped"));
        assert!(app.gateway_server.is_none());

        // Turn off when not running
        command(&mut app, "/turn-off-remote");
        let last = app.transcript.last().unwrap();
        assert!(last.text.contains("not running"));
    }

    #[test]
    fn test_export_command_preview_and_confirmed_export() {
        let (dir, mut app) = app();

        // 1. Show usage
        command(&mut app, "/export");
        let last = app.transcript.last().unwrap();
        assert!(last.text.contains("Usage: /export"));

        // Create a test doc in workspace
        let note_path = dir.path().join("note.md");
        std::fs::write(&note_path, "# Notes\nMeeting summary.").unwrap();

        // 2. Export preview with source
        command(&mut app, "/export docx out.docx note.md");
        let last = app.transcript.last().unwrap();
        assert!(last.text.contains("Export preview for 'out.docx'"));
        assert!(app.pending_export.is_some());

        // 3. Confirm export fails while workspace is read-only
        command(&mut app, "/export confirm out.docx");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Error));
        assert!(last.text.contains("Workspace is read-only"));

        // 4. Grant write permission, preview again, and confirm
        command(&mut app, "/permissions grant");
        command(&mut app, "/export docx out.docx note.md");
        command(&mut app, "/export confirm out.docx");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(last.text.contains("Successfully exported"));
        assert!(dir.path().join("out.docx").exists());
    }

    #[test]
    fn test_tab_completion_slash_commands_and_file() {
        let (dir, mut app) = app();

        // 1. Slash command completion
        app.input = Zeroizing::new("/per".to_string());
        app.handle_tab_completion();
        assert_eq!(app.input.as_str(), "/permissions ");

        // 2. Multiple commands completion
        app.input = Zeroizing::new("/turn".to_string());
        app.handle_tab_completion();
        assert!(app.input.starts_with("/turn-"));
        let last = app.transcript.last().unwrap();
        assert!(last.text.contains("Available commands"));

        // 3. @file completion
        std::fs::write(dir.path().join("document1.md"), "content").unwrap();
        app.input = Zeroizing::new("look at @doc".to_string());
        app.handle_tab_completion();
        assert_eq!(app.input.as_str(), "look at @document1.md ");
    }

    #[test]
    fn test_undo_command_restores_file() {
        let (dir, mut app) = app();
        command(&mut app, "/permissions grant");

        let file_path = dir.path().join("draft.md");
        std::fs::write(&file_path, "Original content").unwrap();

        // Apply an in-place edit using core editing module
        let mut undo_stack = SessionUndoStack::new();
        let _ = dociler_core::editing::apply_in_place_edit(
            app.session.as_ref().unwrap().workspace(),
            WritePolicy::ConfirmEveryWrite,
            true,
            std::path::Path::new("draft.md"),
            "Modified content",
            &mut undo_stack,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "Modified content"
        );

        app.undo_stack = undo_stack;
        app.last_edited_path = Some(std::path::PathBuf::from("draft.md"));

        // Undo
        command(&mut app, "/undo draft.md");
        let last = app.transcript.last().unwrap();
        assert!(matches!(last.kind, EntryKind::Notice));
        assert!(last.text.contains("Restored previous content"));
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "Original content"
        );
    }
}
