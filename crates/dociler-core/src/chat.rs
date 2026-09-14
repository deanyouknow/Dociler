//! Provider-independent, memory-only generation control for terminal and API adapters.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use zeroize::Zeroizing;

pub use crate::cancellation::CancellationToken;
use crate::credentials::{CredentialError, CredentialStore, OsCredentialStore};
use crate::remote::{RemoteClient, RemoteError, RemoteProfile};
use crate::session::{Message, Role, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatError {
    Credential(CredentialError),
    Remote(RemoteError),
    HistoryFull,
    Delivery,
}

/// A backend streams one answer and returns the identical assembled answer.
/// Implementations must observe `cancellation` at safe interruption points.
pub trait ChatBackend: Send + 'static {
    fn stream(
        self: Box<Self>,
        messages: &[Message],
        cancellation: &CancellationToken,
        output: &mut dyn FnMut(&str) -> Result<(), ChatError>,
    ) -> Result<String, ChatError>;
}

pub struct RemoteChatBackend<S = OsCredentialStore> {
    profile: RemoteProfile,
    credentials: S,
}

impl RemoteChatBackend<OsCredentialStore> {
    pub fn native(profile: RemoteProfile) -> Self {
        Self {
            profile,
            credentials: OsCredentialStore,
        }
    }
}

impl<S> RemoteChatBackend<S> {
    pub fn new(profile: RemoteProfile, credentials: S) -> Self {
        Self {
            profile,
            credentials,
        }
    }
}

impl<S: CredentialStore + Send + 'static> ChatBackend for RemoteChatBackend<S> {
    fn stream(
        self: Box<Self>,
        messages: &[Message],
        cancellation: &CancellationToken,
        output: &mut dyn FnMut(&str) -> Result<(), ChatError>,
    ) -> Result<String, ChatError> {
        let secret = if self.profile.needs_credential() {
            self.credentials
                .get(&self.profile.credential_id())
                .map_err(ChatError::Credential)?
                .ok_or(ChatError::Credential(CredentialError::Unavailable))?
                .into()
        } else {
            None
        };
        if cancellation.is_cancelled() {
            return Err(ChatError::Remote(RemoteError::Cancelled));
        }
        let client =
            RemoteClient::connect(self.profile, secret.as_ref()).map_err(ChatError::Remote)?;
        if cancellation.is_cancelled() {
            return Err(ChatError::Remote(RemoteError::Cancelled));
        }
        let mut output_error = None;
        let answer = client
            .stream_chat_cancellable(messages, cancellation, |text| {
                if cancellation.is_cancelled() {
                    return Err(RemoteError::Cancelled);
                }
                if let Err(error) = output(text) {
                    output_error = Some(error);
                    return Err(RemoteError::Output);
                }
                Ok(())
            })
            .map_err(|error| match error {
                RemoteError::Output => output_error.unwrap_or(ChatError::Delivery),
                error => ChatError::Remote(error),
            })?;
        if cancellation.is_cancelled() {
            return Err(ChatError::Remote(RemoteError::Cancelled));
        }
        Ok(answer)
    }
}

pub enum GenerationEvent {
    Token(Zeroizing<String>),
    Finished(Session),
    Failed(Session, ChatError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationPoll {
    Pending,
    Event,
    Disconnected,
}

/// Owns one worker turn. Dropping it requests cancellation but never persists state.
pub struct Generation {
    receiver: Receiver<GenerationEvent>,
    cancellation: CancellationToken,
}

impl Generation {
    /// Returns the unchanged session if the prompt would exceed its memory limit.
    pub fn start(
        mut session: Session,
        prompt: String,
        backend: Box<dyn ChatBackend>,
    ) -> Result<Self, Session> {
        if session.push(Role::User, prompt).is_err() {
            return Err(session);
        }
        let (sender, receiver) = mpsc::channel();
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        thread::spawn(move || {
            if worker_cancellation.is_cancelled() {
                let _ = sender.send(GenerationEvent::Failed(
                    session,
                    ChatError::Remote(RemoteError::Cancelled),
                ));
                return;
            }
            let result = backend.stream(session.messages(), &worker_cancellation, &mut |text| {
                if worker_cancellation.is_cancelled() {
                    return Err(ChatError::Remote(RemoteError::Cancelled));
                }
                sender
                    .send(GenerationEvent::Token(Zeroizing::new(text.to_owned())))
                    .map_err(|_| ChatError::Delivery)
            });
            match result {
                Ok(_) if worker_cancellation.is_cancelled() => {
                    let _ = sender.send(GenerationEvent::Failed(
                        session,
                        ChatError::Remote(RemoteError::Cancelled),
                    ));
                }
                Ok(answer) => {
                    if session.push(Role::Assistant, answer).is_err() {
                        let _ =
                            sender.send(GenerationEvent::Failed(session, ChatError::HistoryFull));
                    } else {
                        let _ = sender.send(GenerationEvent::Finished(session));
                    }
                }
                Err(error) => {
                    let _ = sender.send(GenerationEvent::Failed(session, error));
                }
            }
        });
        Ok(Self {
            receiver,
            cancellation,
        })
    }

    pub fn poll(&self) -> (GenerationPoll, Option<GenerationEvent>) {
        match self.receiver.try_recv() {
            Ok(event) => (GenerationPoll::Event, Some(event)),
            Err(TryRecvError::Empty) => (GenerationPoll::Pending, None),
            Err(TryRecvError::Disconnected) => (GenerationPoll::Disconnected, None),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::workspace::Workspace;

    struct FixedBackend;

    impl ChatBackend for FixedBackend {
        fn stream(
            self: Box<Self>,
            messages: &[Message],
            _: &CancellationToken,
            output: &mut dyn FnMut(&str) -> Result<(), ChatError>,
        ) -> Result<String, ChatError> {
            assert_eq!(messages.len(), 1);
            output("Hel")?;
            output("lo")?;
            Ok("Hello".to_owned())
        }
    }

    struct WaitingBackend(mpsc::Sender<()>);

    impl ChatBackend for WaitingBackend {
        fn stream(
            self: Box<Self>,
            _: &[Message],
            cancellation: &CancellationToken,
            _: &mut dyn FnMut(&str) -> Result<(), ChatError>,
        ) -> Result<String, ChatError> {
            self.0.send(()).unwrap();
            while !cancellation.is_cancelled() {
                thread::yield_now();
            }
            Err(ChatError::Remote(RemoteError::Cancelled))
        }
    }

    fn session() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(dir.path()).unwrap();
        (dir, Session::new(workspace))
    }

    fn next(generation: &Generation) -> GenerationEvent {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let (state, event) = generation.poll();
            if let Some(event) = event {
                return event;
            }
            assert_ne!(state, GenerationPoll::Disconnected);
            assert!(Instant::now() < deadline, "generation event timed out");
            thread::yield_now();
        }
    }

    #[test]
    fn fake_backend_streams_and_commits_one_answer() {
        let (_dir, session) = session();
        let generation = Generation::start(session, "Hi".to_owned(), Box::new(FixedBackend))
            .ok()
            .unwrap();
        assert!(matches!(next(&generation), GenerationEvent::Token(text) if *text == "Hel"));
        assert!(matches!(next(&generation), GenerationEvent::Token(text) if *text == "lo"));
        let GenerationEvent::Finished(session) = next(&generation) else {
            panic!("expected completion");
        };
        assert_eq!(session.messages().len(), 2);
        assert_eq!(session.messages()[0].text(), "Hi");
        assert_eq!(session.messages()[1].text(), "Hello");
    }

    #[test]
    fn cancellation_is_deterministic_and_never_commits_an_answer() {
        let (_dir, session) = session();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let generation = Generation::start(
            session,
            "Sensitive prompt".to_owned(),
            Box::new(WaitingBackend(ready_sender)),
        )
        .ok()
        .unwrap();
        ready_receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        generation.cancel();
        assert!(generation.is_cancel_requested());
        let GenerationEvent::Failed(session, ChatError::Remote(RemoteError::Cancelled)) =
            next(&generation)
        else {
            panic!("expected cancellation");
        };
        assert_eq!(session.messages().len(), 1);
        assert_eq!(session.messages()[0].role(), Role::User);
    }
}
