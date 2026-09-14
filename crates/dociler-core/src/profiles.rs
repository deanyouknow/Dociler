//! Verified remote-profile installation with native-only credential persistence.

use std::io;

use crate::cancellation::CancellationToken;
use crate::config::ConfigStore;
use crate::credentials::{CredentialError, CredentialStore, OsCredentialStore, Secret};
use crate::paths::AppPaths;
use crate::remote::{RemoteClient, RemoteError, RemoteProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileInstallError {
    Config(io::ErrorKind),
    Credential(CredentialError),
    Remote(RemoteError),
    Cancelled,
}

pub trait ProfileVerifier {
    fn verify(
        &self,
        profile: &RemoteProfile,
        secret: Option<&Secret>,
        cancellation: &CancellationToken,
    ) -> Result<(), RemoteError>;
}

pub struct NetworkProfileVerifier;

impl ProfileVerifier for NetworkProfileVerifier {
    fn verify(
        &self,
        profile: &RemoteProfile,
        secret: Option<&Secret>,
        cancellation: &CancellationToken,
    ) -> Result<(), RemoteError> {
        RemoteClient::connect(profile.clone(), secret)?.verify_cancellable(cancellation)
    }
}

pub fn install_remote_profile(
    paths: &AppPaths,
    name: &str,
    url: &str,
    model: &str,
    secret: Option<Secret>,
    cancellation: &CancellationToken,
) -> Result<RemoteProfile, ProfileInstallError> {
    install_remote_profile_with(
        paths,
        name,
        url,
        model,
        secret,
        cancellation,
        &OsCredentialStore,
        &NetworkProfileVerifier,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn install_remote_profile_with<S: CredentialStore, V: ProfileVerifier>(
    paths: &AppPaths,
    name: &str,
    url: &str,
    model: &str,
    secret: Option<Secret>,
    cancellation: &CancellationToken,
    credentials: &S,
    verifier: &V,
) -> Result<RemoteProfile, ProfileInstallError> {
    if cancellation.is_cancelled() {
        return Err(ProfileInstallError::Cancelled);
    }
    let store = ConfigStore::new(paths.clone());
    let mut loaded = store
        .load()
        .map_err(|error| ProfileInstallError::Config(error.kind()))?;
    let profile = RemoteProfile::new(name, url, model, secret.is_some())
        .map_err(ProfileInstallError::Remote)?;
    loaded
        .settings
        .add_remote_profile(profile.clone())
        .map_err(|error| ProfileInstallError::Config(error.kind()))?;
    if let Err(error) = verifier.verify(&profile, secret.as_ref(), cancellation) {
        return if cancellation.is_cancelled() || error == RemoteError::Cancelled {
            Err(ProfileInstallError::Cancelled)
        } else {
            Err(ProfileInstallError::Remote(error))
        };
    }
    if cancellation.is_cancelled() {
        return Err(ProfileInstallError::Cancelled);
    }

    let credential_id = profile.credential_id();
    if let Some(secret) = &secret {
        credentials
            .set(&credential_id, secret)
            .map_err(ProfileInstallError::Credential)?;
    }
    if cancellation.is_cancelled() {
        if secret.is_some() {
            let _ = credentials.delete(&credential_id);
        }
        return Err(ProfileInstallError::Cancelled);
    }
    if let Err(error) = store.save(&loaded.settings) {
        if secret.is_some() {
            let _ = credentials.delete(&credential_id);
        }
        return Err(ProfileInstallError::Config(error.kind()));
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryCredentials(Mutex<Option<String>>);

    impl CredentialStore for MemoryCredentials {
        fn get(
            &self,
            _: &crate::credentials::CredentialId,
        ) -> Result<Option<Secret>, CredentialError> {
            Ok(self.0.lock().unwrap().clone().map(Secret::new))
        }

        fn set(
            &self,
            _: &crate::credentials::CredentialId,
            secret: &Secret,
        ) -> Result<(), CredentialError> {
            *self.0.lock().unwrap() = Some(secret.expose().to_owned());
            Ok(())
        }

        fn delete(&self, _: &crate::credentials::CredentialId) -> Result<(), CredentialError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    struct Accept;

    impl ProfileVerifier for Accept {
        fn verify(
            &self,
            _: &RemoteProfile,
            _: Option<&Secret>,
            _: &CancellationToken,
        ) -> Result<(), RemoteError> {
            Ok(())
        }
    }

    struct CancelDuringVerification;

    impl ProfileVerifier for CancelDuringVerification {
        fn verify(
            &self,
            _: &RemoteProfile,
            _: Option<&Secret>,
            cancellation: &CancellationToken,
        ) -> Result<(), RemoteError> {
            cancellation.cancel();
            Err(RemoteError::Cancelled)
        }
    }

    fn paths(dir: &tempfile::TempDir) -> AppPaths {
        AppPaths::new(
            dir.path().join("config"),
            dir.path().join("data"),
            dir.path().join("cache"),
        )
        .unwrap()
    }

    #[test]
    fn verified_profile_and_secret_are_saved_to_separate_stores() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        let credentials = MemoryCredentials::default();
        let cancellation = CancellationToken::new();
        let profile = install_remote_profile_with(
            &paths,
            "office",
            "https://example.com/v1",
            "model",
            Some(Secret::new("secret-value".to_owned())),
            &cancellation,
            &credentials,
            &Accept,
        )
        .unwrap();

        assert!(profile.needs_credential());
        assert_eq!(
            credentials.0.lock().unwrap().as_deref(),
            Some("secret-value")
        );
        let saved = ConfigStore::new(paths).load().unwrap();
        let saved = saved.settings.remote_profile("office").unwrap();
        assert_eq!(saved.base_url(), "https://example.com/v1/");
        let config = std::fs::read_to_string(dir.path().join("config/config.json")).unwrap();
        assert!(!config.contains("secret-value"));
    }

    #[test]
    fn pre_cancelled_install_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = install_remote_profile_with(
            &paths,
            "office",
            "https://example.com/v1",
            "model",
            None,
            &cancellation,
            &MemoryCredentials::default(),
            &Accept,
        )
        .unwrap_err();
        assert_eq!(error, ProfileInstallError::Cancelled);
        assert!(!paths.config_dir.exists());
    }

    #[test]
    fn cancellation_during_verification_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        let cancellation = CancellationToken::new();
        let error = install_remote_profile_with(
            &paths,
            "office",
            "https://example.com/v1",
            "model",
            Some(Secret::new("not-persisted".to_owned())),
            &cancellation,
            &MemoryCredentials::default(),
            &CancelDuringVerification,
        )
        .unwrap_err();
        assert_eq!(error, ProfileInstallError::Cancelled);
        assert!(!paths.config_dir.exists());
    }
}
