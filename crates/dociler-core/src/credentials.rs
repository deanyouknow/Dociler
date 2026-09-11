//! Credential boundary. No file-backed fallback or OS adapter is supplied yet.

use std::fmt;
use zeroize::Zeroizing;

/// Explicit secret access only; debug output never includes the value.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Opaque service/profile identifier, never an API key or remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialId(String);

impl CredentialId {
    pub fn new(id: &str) -> Result<Self, CredentialError> {
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(CredentialError::InvalidId);
        }
        Ok(Self(id.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    Unavailable,
    AccessDenied,
    InvalidId,
}

pub trait CredentialStore {
    fn get(&self, id: &CredentialId) -> Result<Option<Secret>, CredentialError>;
    fn set(&self, id: &CredentialId, secret: &Secret) -> Result<(), CredentialError>;
    fn delete(&self, id: &CredentialId) -> Result<(), CredentialError>;
}

/// Until a native adapter exists, fail closed instead of storing plaintext.
pub struct UnavailableCredentialStore;

impl CredentialStore for UnavailableCredentialStore {
    fn get(&self, _: &CredentialId) -> Result<Option<Secret>, CredentialError> {
        Err(CredentialError::Unavailable)
    }
    fn set(&self, _: &CredentialId, _: &Secret) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable)
    }
    fn delete(&self, _: &CredentialId) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable)
    }
}
