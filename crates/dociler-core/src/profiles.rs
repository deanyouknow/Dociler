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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileMutationError {
    Config(io::ErrorKind),
    Credential(CredentialError),
    Remote(RemoteError),
    Cancelled,
    NotFound,
    CredentialRollback(CredentialError),
}

pub struct ProfileRemoval {
    profiles: Vec<RemoteProfile>,
    credential_cleanup_warning: Option<CredentialError>,
}

impl ProfileRemoval {
    pub fn profiles(&self) -> &[RemoteProfile] {
        &self.profiles
    }

    pub fn credential_cleanup_warning(&self) -> Option<CredentialError> {
        self.credential_cleanup_warning
    }
}

pub struct ProfileRotation {
    profile: RemoteProfile,
    profiles: Vec<RemoteProfile>,
    credential_cleanup_warning: Option<CredentialError>,
}

impl ProfileRotation {
    pub fn profile(&self) -> &RemoteProfile {
        &self.profile
    }

    pub fn profiles(&self) -> &[RemoteProfile] {
        &self.profiles
    }

    pub fn credential_cleanup_warning(&self) -> Option<CredentialError> {
        self.credential_cleanup_warning
    }
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

pub fn remove_remote_profile(
    paths: &AppPaths,
    name: &str,
    cancellation: &CancellationToken,
) -> Result<ProfileRemoval, ProfileMutationError> {
    remove_remote_profile_with(paths, name, cancellation, &OsCredentialStore)
}

pub fn remove_remote_profile_with<S: CredentialStore>(
    paths: &AppPaths,
    name: &str,
    cancellation: &CancellationToken,
    credentials: &S,
) -> Result<ProfileRemoval, ProfileMutationError> {
    if cancellation.is_cancelled() {
        return Err(ProfileMutationError::Cancelled);
    }
    let store = ConfigStore::new(paths.clone());
    let mut loaded = store
        .load()
        .map_err(|error| ProfileMutationError::Config(error.kind()))?;
    let profile = loaded
        .settings
        .remove_remote_profile(name)
        .ok_or(ProfileMutationError::NotFound)?;
    if cancellation.is_cancelled() {
        return Err(ProfileMutationError::Cancelled);
    }
    store
        .save(&loaded.settings)
        .map_err(|error| ProfileMutationError::Config(error.kind()))?;
    let credential_cleanup_warning = if profile.needs_credential() {
        credentials.delete(&profile.credential_id()).err()
    } else {
        None
    };
    Ok(ProfileRemoval {
        profiles: loaded.settings.remote_profiles().to_vec(),
        credential_cleanup_warning,
    })
}

pub fn rotate_remote_credential(
    paths: &AppPaths,
    name: &str,
    secret: Option<Secret>,
    cancellation: &CancellationToken,
) -> Result<ProfileRotation, ProfileMutationError> {
    rotate_remote_credential_with(
        paths,
        name,
        secret,
        cancellation,
        &OsCredentialStore,
        &NetworkProfileVerifier,
    )
}

pub fn rotate_remote_credential_with<S: CredentialStore, V: ProfileVerifier>(
    paths: &AppPaths,
    name: &str,
    secret: Option<Secret>,
    cancellation: &CancellationToken,
    credentials: &S,
    verifier: &V,
) -> Result<ProfileRotation, ProfileMutationError> {
    if cancellation.is_cancelled() {
        return Err(ProfileMutationError::Cancelled);
    }
    let store = ConfigStore::new(paths.clone());
    let mut loaded = store
        .load()
        .map_err(|error| ProfileMutationError::Config(error.kind()))?;
    let current = loaded
        .settings
        .remote_profile(name)
        .cloned()
        .ok_or(ProfileMutationError::NotFound)?;
    let candidate = current.with_credential(secret.is_some());
    if let Err(error) = verifier.verify(&candidate, secret.as_ref(), cancellation) {
        return if cancellation.is_cancelled() || error == RemoteError::Cancelled {
            Err(ProfileMutationError::Cancelled)
        } else {
            Err(ProfileMutationError::Remote(error))
        };
    }
    if cancellation.is_cancelled() {
        return Err(ProfileMutationError::Cancelled);
    }

    let credential_id = current.credential_id();
    let credential_cleanup_warning;
    if let Some(secret) = &secret {
        let old_secret = if current.needs_credential() {
            credentials
                .get(&credential_id)
                .map_err(ProfileMutationError::Credential)?
        } else {
            None
        };
        credentials
            .set(&credential_id, secret)
            .map_err(ProfileMutationError::Credential)?;
        if cancellation.is_cancelled() {
            let rollback = match old_secret.as_ref() {
                Some(old_secret) => credentials.set(&credential_id, old_secret),
                None => credentials.delete(&credential_id),
            };
            return match rollback {
                Ok(()) => Err(ProfileMutationError::Cancelled),
                Err(rollback) => Err(ProfileMutationError::CredentialRollback(rollback)),
            };
        }
        if !current.needs_credential() {
            let config_result = loaded
                .settings
                .set_remote_profile_credential(name, true)
                .and_then(|_| store.save(&loaded.settings));
            if let Err(error) = config_result {
                let rollback = match old_secret.as_ref() {
                    Some(old_secret) => credentials.set(&credential_id, old_secret),
                    None => credentials.delete(&credential_id),
                };
                return match rollback {
                    Ok(()) => Err(ProfileMutationError::Config(error.kind())),
                    Err(rollback) => Err(ProfileMutationError::CredentialRollback(rollback)),
                };
            }
        }
        credential_cleanup_warning = None;
    } else if current.needs_credential() {
        loaded
            .settings
            .set_remote_profile_credential(name, false)
            .map_err(|error| ProfileMutationError::Config(error.kind()))?;
        store
            .save(&loaded.settings)
            .map_err(|error| ProfileMutationError::Config(error.kind()))?;
        credential_cleanup_warning = credentials.delete(&credential_id).err();
    } else {
        credential_cleanup_warning = None;
    }

    Ok(ProfileRotation {
        profile: candidate,
        profiles: loaded.settings.remote_profiles().to_vec(),
        credential_cleanup_warning,
    })
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

    struct DeleteDenied;

    impl CredentialStore for DeleteDenied {
        fn get(
            &self,
            _: &crate::credentials::CredentialId,
        ) -> Result<Option<Secret>, CredentialError> {
            Ok(None)
        }

        fn set(
            &self,
            _: &crate::credentials::CredentialId,
            _: &Secret,
        ) -> Result<(), CredentialError> {
            Ok(())
        }

        fn delete(&self, _: &crate::credentials::CredentialId) -> Result<(), CredentialError> {
            Err(CredentialError::AccessDenied)
        }
    }

    struct CancelOnSet {
        cancellation: CancellationToken,
        value: Mutex<Option<String>>,
    }

    impl CredentialStore for CancelOnSet {
        fn get(
            &self,
            _: &crate::credentials::CredentialId,
        ) -> Result<Option<Secret>, CredentialError> {
            Ok(self.value.lock().unwrap().clone().map(Secret::new))
        }

        fn set(
            &self,
            _: &crate::credentials::CredentialId,
            secret: &Secret,
        ) -> Result<(), CredentialError> {
            *self.value.lock().unwrap() = Some(secret.expose().to_owned());
            self.cancellation.cancel();
            Ok(())
        }

        fn delete(&self, _: &crate::credentials::CredentialId) -> Result<(), CredentialError> {
            *self.value.lock().unwrap() = None;
            Ok(())
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

    fn save_profile(paths: &AppPaths, credential: bool) {
        let store = ConfigStore::new(paths.clone());
        let mut settings = store.load().unwrap().settings;
        settings
            .add_remote_profile(
                RemoteProfile::new("office", "https://example.com/v1", "model", credential)
                    .unwrap(),
            )
            .unwrap();
        store.save(&settings).unwrap();
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

    #[test]
    fn removal_publishes_config_before_deleting_the_credential() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        save_profile(&paths, true);
        let credentials = MemoryCredentials::default();
        credentials
            .set(
                &RemoteProfile::new("office", "https://example.com/v1", "model", true)
                    .unwrap()
                    .credential_id(),
                &Secret::new("old-secret".to_owned()),
            )
            .unwrap();

        let outcome =
            remove_remote_profile_with(&paths, "office", &CancellationToken::new(), &credentials)
                .unwrap();
        assert!(outcome.profiles().is_empty());
        assert_eq!(outcome.credential_cleanup_warning(), None);
        assert!(credentials.0.lock().unwrap().is_none());
        assert!(
            ConfigStore::new(paths)
                .load()
                .unwrap()
                .settings
                .remote_profiles()
                .is_empty()
        );
    }

    #[test]
    fn removal_reports_orphan_cleanup_without_restoring_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        save_profile(&paths, true);

        let outcome =
            remove_remote_profile_with(&paths, "office", &CancellationToken::new(), &DeleteDenied)
                .unwrap();
        assert_eq!(
            outcome.credential_cleanup_warning(),
            Some(CredentialError::AccessDenied)
        );
        assert!(
            ConfigStore::new(paths)
                .load()
                .unwrap()
                .settings
                .remote_profiles()
                .is_empty()
        );
    }

    #[test]
    fn credential_rotation_verifies_then_updates_marker_and_native_store() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        save_profile(&paths, false);
        let credentials = MemoryCredentials::default();
        let cancellation = CancellationToken::new();

        let added = rotate_remote_credential_with(
            &paths,
            "office",
            Some(Secret::new("new-secret".to_owned())),
            &cancellation,
            &credentials,
            &Accept,
        )
        .unwrap();
        assert!(added.profile().needs_credential());
        assert_eq!(credentials.0.lock().unwrap().as_deref(), Some("new-secret"));
        assert!(
            ConfigStore::new(paths.clone())
                .load()
                .unwrap()
                .settings
                .remote_profile("office")
                .unwrap()
                .needs_credential()
        );

        let removed = rotate_remote_credential_with(
            &paths,
            "office",
            None,
            &cancellation,
            &credentials,
            &Accept,
        )
        .unwrap();
        assert!(!removed.profile().needs_credential());
        assert_eq!(removed.credential_cleanup_warning(), None);
        assert!(credentials.0.lock().unwrap().is_none());
        assert!(
            !ConfigStore::new(paths)
                .load()
                .unwrap()
                .settings
                .remote_profile("office")
                .unwrap()
                .needs_credential()
        );
    }

    #[test]
    fn pre_cancelled_profile_mutations_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        save_profile(&paths, false);
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(matches!(
            remove_remote_profile_with(
                &paths,
                "office",
                &cancellation,
                &MemoryCredentials::default(),
            ),
            Err(ProfileMutationError::Cancelled)
        ));
        assert!(matches!(
            rotate_remote_credential_with(
                &paths,
                "office",
                Some(Secret::new("unused".to_owned())),
                &cancellation,
                &MemoryCredentials::default(),
                &Accept,
            ),
            Err(ProfileMutationError::Cancelled)
        ));
        assert!(
            ConfigStore::new(paths)
                .load()
                .unwrap()
                .settings
                .remote_profile("office")
                .is_some()
        );
    }

    #[test]
    fn cancellation_during_new_credential_write_rolls_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        save_profile(&paths, false);
        let cancellation = CancellationToken::new();
        let credentials = CancelOnSet {
            cancellation: cancellation.clone(),
            value: Mutex::new(None),
        };

        assert!(matches!(
            rotate_remote_credential_with(
                &paths,
                "office",
                Some(Secret::new("temporary".to_owned())),
                &cancellation,
                &credentials,
                &Accept,
            ),
            Err(ProfileMutationError::Cancelled)
        ));
        assert!(credentials.value.lock().unwrap().is_none());
        assert!(
            !ConfigStore::new(paths)
                .load()
                .unwrap()
                .settings
                .remote_profile("office")
                .unwrap()
                .needs_credential()
        );
    }
}
