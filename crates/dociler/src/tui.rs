use std::io;
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
use dociler_core::chat::{
    CancellationToken, ChatError, Generation, GenerationEvent, GenerationPoll, RemoteChatBackend,
};
use dociler_core::credentials::{CredentialError, Secret};
use dociler_core::paths::AppPaths;
use dociler_core::profiles::{
    ProfileEdit, ProfileInstallError, ProfileMutationError, ProfileRemoval, ProfileRotation,
    check_remote_profile, edit_remote_profile, install_remote_profile, load_remote_profiles,
    remove_remote_profile, rotate_remote_credential,
};
use dociler_core::remote::{RemoteError, RemoteProfile};
use dociler_core::session::Session;
use dociler_core::workspace::Workspace;
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
    connection_status: ConnectionStatus,
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
            connection_status: ConnectionStatus::Unknown,
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

    fn selected_profile(&self) -> Option<&RemoteProfile> {
        self.selected.and_then(|index| self.profiles.get(index))
    }

    fn active_profile_is(&self, name: &str) -> bool {
        self.selected_profile()
            .is_some_and(|profile| profile.name() == name)
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
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or("");
        match command {
            "/help" => self.notice(
                "/help  /status  /connect [NAME|add|refresh|check NAME|edit NAME|remove NAME|key NAME]  /clear  /exit\nProfile edits and keys are verified before commit; removal requires exact repetition. Esc cancels active work; PageUp/PageDown scroll.",
            ),
            "/status" => {
                let profile = self
                    .selected_profile()
                    .map(|profile| format!("{} ({})", profile.name(), profile.model()))
                    .unwrap_or_else(|| "none".to_owned());
                self.notice(format!(
                    "Workspace: {}\nBackend: Remote\nProfile: {profile}\nConnection: {}\nAccess: read-only\nHistory: memory-only\nLAN API: off",
                    safe_path(self.workspace.root()),
                    self.connection_status_text()
                ));
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
            "/model" | "/files" | "/permissions" | "/export" | "/turn-on-remote"
            | "/turn-off-remote" | "/update" => self.error(
                "That command is planned but unavailable in this text-only milestone.",
            ),
            _ => self.error("Unknown command. Use /help."),
        }
    }

    fn start_generation(&mut self, prompt: String) {
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
                    self.quit = true
                } else {
                    self.cancel()
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

fn safe_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .chars()
        .flat_map(char::escape_default)
        .collect()
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

    let profile = app
        .selected_profile()
        .map(|profile| format!("{} · {}", profile.name(), profile.model()))
        .unwrap_or_else(|| "no remote profile".to_owned());
    let connection = app.connection_badge();
    let header = Paragraph::new(vec![
        Line::from(Span::styled(
            "DOCILER",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "Remote · {profile} · {connection} · read-only · memory-only · API off"
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
        command(&mut app, "/files");
        assert!(matches!(
            app.transcript.last().unwrap().kind,
            EntryKind::Error
        ));
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
}
