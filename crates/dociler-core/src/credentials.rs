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

/// The native user credential service: Keychain on macOS, Credential Manager
/// on Windows, and Secret Service on Linux. It has no plaintext fallback.
pub struct OsCredentialStore;

impl OsCredentialStore {
    fn entry(id: &CredentialId) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new("io.dociler.remote", id.as_str()).map_err(map_error)
    }
}

fn map_error(error: keyring::Error) -> CredentialError {
    match error {
        keyring::Error::NoEntry => CredentialError::Unavailable,
        _ => CredentialError::AccessDenied,
    }
}

impl CredentialStore for OsCredentialStore {
    fn get(&self, id: &CredentialId) -> Result<Option<Secret>, CredentialError> {
        match Self::entry(id)?.get_password() {
            Ok(secret) => Ok(Some(Secret::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_error(error)),
        }
    }

    fn set(&self, id: &CredentialId, secret: &Secret) -> Result<(), CredentialError> {
        Self::entry(id)?
            .set_password(secret.expose())
            .map_err(map_error)
    }

    fn delete(&self, id: &CredentialId) -> Result<(), CredentialError> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_error(error)),
        }
    }
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
