//! Bounded memory-only conversation foundation; no inference or persistence.

use crate::workspace::Workspace;
use zeroize::Zeroizing;

const MAX_HISTORY_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

// Intentionally neither Debug nor Serialize: message text is sensitive.
pub struct Message {
    role: Role,
    text: Zeroizing<String>,
}

impl Message {
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn text(&self) -> &str {
        &self.text
    }
}

pub struct Session {
    workspace: Workspace,
    messages: Vec<Message>,
    history_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryFull;

impl Session {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            messages: Vec::new(),
            history_bytes: 0,
        }
    }
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
    pub fn push(&mut self, role: Role, text: String) -> Result<(), HistoryFull> {
        let text = Zeroizing::new(text);
        if self.messages.len() >= MAX_MESSAGES
            || text.len() > MAX_HISTORY_BYTES - self.history_bytes
        {
            return Err(HistoryFull);
        }
        self.history_bytes += text.len();
        self.messages.push(Message { role, text });
        Ok(())
    }
    pub fn clear(&mut self) {
        self.messages.clear();
        self.history_bytes = 0;
    }
}
