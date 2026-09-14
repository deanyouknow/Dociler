use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
use dociler_core::credentials::{CredentialError, CredentialStore, OsCredentialStore};
use dociler_core::remote::{RemoteClient, RemoteError, RemoteProfile};
use dociler_core::session::{Role, Session};
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

#[derive(Clone, Copy)]
enum WorkerError {
    Credential(CredentialError),
    Remote(RemoteError),
}

enum WorkerEvent {
    Token(String),
    Finished(Session),
    Failed(Session, WorkerError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Ready,
    Generating,
    Cancelling,
}

pub struct App {
    workspace: Workspace,
    profiles: Vec<RemoteProfile>,
    selected: Option<usize>,
    session: Option<Session>,
    transcript: Vec<TranscriptEntry>,
    input: Zeroizing<String>,
    partial_answer: Zeroizing<String>,
    worker: Option<Receiver<WorkerEvent>>,
    cancel: Option<Arc<AtomicBool>>,
    activity: Activity,
    scroll: u16,
    confirm_clear: bool,
    quit: bool,
}

impl App {
    pub fn new(
        workspace: Workspace,
        profiles: Vec<RemoteProfile>,
        selected_name: Option<&str>,
    ) -> Self {
        let selected = selected_name
            .and_then(|name| profiles.iter().position(|profile| profile.name() == name))
            .or_else(|| (!profiles.is_empty()).then_some(0));
        let mut app = Self {
            session: Some(Session::new(workspace.clone())),
            workspace,
            profiles,
            selected,
            transcript: Vec::new(),
            input: Zeroizing::new(String::new()),
            partial_answer: Zeroizing::new(String::new()),
            worker: None,
            cancel: None,
            activity: Activity::Ready,
            scroll: 0,
            confirm_clear: false,
            quit: false,
        };
        app.notice("Welcome to Dociler. Text chat is memory-only; documents and local models are not active.");
        if app.selected.is_none() {
            app.notice("No remote profile is saved. Exit and use `dociler connect add NAME URL MODEL`, then relaunch.");
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

    fn submit(&mut self) {
        if self.activity != Activity::Ready {
            self.error("A response is already active. Press Esc to cancel it.");
            return;
        }
        let text = self.input.trim().to_owned();
        self.input.zeroize();
        self.input.clear();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') {
            self.handle_command(&text);
        } else {
            self.start_generation(text);
        }
    }

    fn handle_command(&mut self, input: &str) {
        let confirmed_clear = self.confirm_clear;
        self.confirm_clear = false;
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or("");
        match command {
            "/help" => self.notice(
                "/help  /status  /connect [NAME]  /clear  /exit\nEsc cancels a response; PageUp/PageDown scroll. Other planned commands are shown as unavailable.",
            ),
            "/status" => {
                let profile = self
                    .selected_profile()
                    .map(|profile| format!("{} ({})", profile.name(), profile.model()))
                    .unwrap_or_else(|| "none".to_owned());
                self.notice(format!(
                    "Workspace: {}\nBackend: Remote\nProfile: {profile}\nAccess: read-only\nHistory: memory-only\nLAN API: off",
                    safe_path(self.workspace.root())
                ));
            }
            "/connect" => {
                if let Some(name) = parts.next() {
                    if parts.next().is_some() {
                        self.error("Usage: /connect NAME");
                        return;
                    }
                    if let Some(index) = self.profiles.iter().position(|profile| profile.name() == name) {
                        self.selected = Some(index);
                        self.session = Some(Session::new(self.workspace.clone()));
                        self.confirm_clear = false;
                        self.notice(format!("Switched to {name}. Conversation context was reset."));
                    } else {
                        self.error("Unknown profile. Use /connect to list saved profiles.");
                    }
                } else if self.profiles.is_empty() {
                    self.notice("No profiles saved. Use `dociler connect add NAME URL MODEL` outside this screen.");
                } else {
                    let profiles = self
                        .profiles
                        .iter()
                        .map(|profile| format!("{} ({})", profile.name(), profile.model()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.notice(format!("Saved profiles:\n{profiles}"));
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
        let Some(profile) = self.selected_profile().cloned() else {
            self.error("No active remote profile. Use /connect or add one outside this screen.");
            return;
        };
        let Some(mut session) = self.session.take() else {
            self.error("Conversation state is unavailable; use /clear before retrying.");
            return;
        };
        if session.push(Role::User, prompt.clone()).is_err() {
            self.session = Some(session);
            self.error("Conversation memory limit reached. Use /clear before continuing.");
            return;
        }
        self.transcript
            .push(TranscriptEntry::new(EntryKind::User, prompt));
        self.partial_answer.zeroize();
        self.partial_answer.clear();
        self.confirm_clear = false;
        self.activity = Activity::Generating;
        self.scroll = u16::MAX;

        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            let secret = if profile.needs_credential() {
                match OsCredentialStore.get(&profile.credential_id()) {
                    Ok(Some(secret)) => Some(secret),
                    Ok(None) => {
                        let _ = sender.send(WorkerEvent::Failed(
                            session,
                            WorkerError::Credential(CredentialError::Unavailable),
                        ));
                        return;
                    }
                    Err(error) => {
                        let _ = sender
                            .send(WorkerEvent::Failed(session, WorkerError::Credential(error)));
                        return;
                    }
                }
            } else {
                None
            };
            if worker_cancel.load(Ordering::Relaxed) {
                let _ = sender.send(WorkerEvent::Failed(
                    session,
                    WorkerError::Remote(RemoteError::Cancelled),
                ));
                return;
            }
            let client = match RemoteClient::connect(profile, secret.as_ref()) {
                Ok(client) => client,
                Err(error) => {
                    let _ = sender.send(WorkerEvent::Failed(session, WorkerError::Remote(error)));
                    return;
                }
            };
            if worker_cancel.load(Ordering::Relaxed) {
                let _ = sender.send(WorkerEvent::Failed(
                    session,
                    WorkerError::Remote(RemoteError::Cancelled),
                ));
                return;
            }
            let answer = client.stream_chat(session.messages(), |text| {
                if worker_cancel.load(Ordering::Relaxed) {
                    return Err(RemoteError::Cancelled);
                }
                sender
                    .send(WorkerEvent::Token(text.to_owned()))
                    .map_err(|_| RemoteError::Cancelled)
            });
            match answer {
                Ok(_) if worker_cancel.load(Ordering::Relaxed) => {
                    let _ = sender.send(WorkerEvent::Failed(
                        session,
                        WorkerError::Remote(RemoteError::Cancelled),
                    ));
                }
                Ok(answer) => {
                    if session.push(Role::Assistant, answer).is_err() {
                        let _ = sender.send(WorkerEvent::Failed(
                            session,
                            WorkerError::Remote(RemoteError::ResponseLimit),
                        ));
                    } else {
                        let _ = sender.send(WorkerEvent::Finished(session));
                    }
                }
                Err(error) => {
                    let _ = sender.send(WorkerEvent::Failed(session, WorkerError::Remote(error)));
                }
            }
        });
        self.worker = Some(receiver);
        self.cancel = Some(cancel);
    }

    fn tick(&mut self) {
        loop {
            let event = match self.worker.as_ref().map(Receiver::try_recv) {
                Some(Ok(event)) => event,
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.worker = None;
                    self.cancel = None;
                    self.activity = Activity::Ready;
                    self.session = Some(Session::new(self.workspace.clone()));
                    self.error(
                        "The response worker ended unexpectedly; conversation context was reset.",
                    );
                    break;
                }
            };
            match event {
                WorkerEvent::Token(text) => {
                    self.partial_answer.push_str(&text);
                    self.scroll = u16::MAX;
                }
                WorkerEvent::Finished(session) => {
                    self.finish_partial();
                    self.session = Some(session);
                    self.worker = None;
                    self.cancel = None;
                    self.activity = Activity::Ready;
                    break;
                }
                WorkerEvent::Failed(session, error) => {
                    self.finish_partial();
                    self.session = Some(session);
                    self.worker = None;
                    self.cancel = None;
                    self.activity = Activity::Ready;
                    self.error(worker_error_message(error));
                    break;
                }
            }
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
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.activity = Activity::Cancelling;
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

fn worker_error_message(error: WorkerError) -> &'static str {
    match error {
        WorkerError::Credential(CredentialError::Unavailable) => {
            "Credential missing or OS credential service unavailable."
        }
        WorkerError::Credential(_) => {
            "OS credential service denied access; no plaintext fallback was used."
        }
        WorkerError::Remote(RemoteError::Cancelled) => {
            "Response cancelled. Partial text, if any, is not added to model context."
        }
        WorkerError::Remote(RemoteError::UnsafeEndpoint) => {
            "Endpoint failed the HTTPS/private-address policy."
        }
        WorkerError::Remote(RemoteError::Authentication) => "Remote authentication failed.",
        WorkerError::Remote(RemoteError::ModelUnavailable) => {
            "The configured upstream model is unavailable."
        }
        WorkerError::Remote(RemoteError::ResponseLimit) => {
            "Remote response exceeded a safety limit."
        }
        WorkerError::Remote(RemoteError::Connection | RemoteError::Resolution) => {
            "Remote connection or DNS resolution failed."
        }
        WorkerError::Remote(RemoteError::Upstream) => {
            "Remote endpoint returned an error or redirect."
        }
        WorkerError::Remote(RemoteError::InvalidProfile | RemoteError::InvalidResponse) => {
            "Remote profile or response was invalid."
        }
        WorkerError::Remote(RemoteError::Output) => "Internal response delivery failed.",
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
    let header = Paragraph::new(vec![
        Line::from(Span::styled(
            "DOCILER",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "Remote · {profile} · read-only · memory-only · API off"
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

    let input_line_count = app.input.lines().count().max(1);
    let input_scroll = input_line_count.saturating_sub(2) as u16;
    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Message · Enter send · Alt+Enter newline "),
            )
            .scroll((input_scroll, 0))
            .wrap(Wrap { trim: false }),
        areas[2],
    );
    let last_line = app.input.rsplit('\n').next().unwrap_or("");
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
    profiles: Vec<RemoteProfile>,
    selected_name: Option<&str>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(workspace, profiles, selected_name);
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
        let profiles = vec![
            RemoteProfile::new("one", "https://one.example", "model-1", false).unwrap(),
            RemoteProfile::new("two", "https://two.example", "model-2", false).unwrap(),
        ];
        (dir, App::new(workspace, profiles, Some("one")))
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
        let mut app = App::new(workspace, Vec::new(), None);
        command(&mut app, "hello");
        assert_eq!(app.activity, Activity::Ready);
        assert!(app.worker.is_none());
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
        let cancel = Arc::new(AtomicBool::new(false));
        app.cancel = Some(Arc::clone(&cancel));
        app.activity = Activity::Generating;

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(app.activity, Activity::Cancelling);
        assert!(!app.quit);
    }
}
