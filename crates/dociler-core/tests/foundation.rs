use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use dociler_core::config::{ConfigSource, ConfigStore, LocalProfile, Settings};
use dociler_core::credentials::{
    CredentialError, CredentialId, CredentialStore, Secret, UnavailableCredentialStore,
};
use dociler_core::paths::AppPaths;
use dociler_core::session::{Role, Session};
use dociler_core::workspace::{Workspace, WritePolicy};

fn paths(root: &std::path::Path) -> AppPaths {
    AppPaths::new(root.join("config"), root.join("data"), root.join("cache")).unwrap()
}

#[test]
fn path_resolution_and_default_loading_create_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    assert_eq!(paths.config_file(), dir.path().join("config/config.json"));
    assert_eq!(paths.models_dir(), dir.path().join("data/models"));
    assert_eq!(paths.runtimes_dir(), dir.path().join("data/runtimes"));
    let loaded = ConfigStore::new(paths).load().unwrap();
    assert_eq!(loaded.source, ConfigSource::Defaults);
    assert_eq!(
        loaded.settings.preferred_local_profile(),
        LocalProfile::Lite
    );
    assert_eq!(
        loaded
            .settings
            .write_policy(&Workspace::open(dir.path()).unwrap()),
        WritePolicy::ReadOnly
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn relative_or_empty_app_paths_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for path in [PathBuf::new(), PathBuf::from("settings")] {
        assert!(AppPaths::new(path.clone(), dir.path().into(), dir.path().into()).is_err());
        assert!(AppPaths::new(dir.path().into(), path.clone(), dir.path().into()).is_err());
        assert!(AppPaths::new(dir.path().into(), dir.path().into(), path).is_err());
    }
}

#[test]
fn initialization_is_explicit_private_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    let store = ConfigStore::new(paths.clone());
    store.initialize().unwrap();
    let before = fs::read(paths.config_file()).unwrap();
    assert_eq!(store.load().unwrap().source, ConfigSource::Saved);
    assert_eq!(
        store.initialize().unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(paths.config_file()).unwrap(), before);
    assert_eq!(fs::read_dir(&paths.config_dir).unwrap().count(), 1);
    assert!(!paths.data_dir.exists());
    assert!(!paths.cache_dir.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(paths.config_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(paths.config_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[test]
fn concurrent_initialization_has_one_winner_and_no_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    let left_paths = paths.clone();
    let right_paths = paths.clone();
    let left = std::thread::spawn(move || ConfigStore::new(left_paths).initialize());
    let right = std::thread::spawn(move || ConfigStore::new(right_paths).initialize());
    let results = [left.join().unwrap(), right.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results.into_iter().find_map(Result::err).unwrap().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read_dir(&paths.config_dir).unwrap().count(), 1);
    assert_eq!(
        ConfigStore::new(paths).load().unwrap().source,
        ConfigSource::Saved
    );
}

#[test]
fn invalid_security_settings_fail_closed_without_echoing_values() {
    for bytes in [
        br#"{}"#.as_slice(),
        br#"{"schema_version":2}"#,
        br#"{"schema_version":1,"schema_version":1}"#,
        br#"{"schema_version":1,"api_key":"private-test-value"}"#,
        br#"{"schema_version":1,"preferred_local_profile":"private-test-value"}"#,
        br#"{"schema_version":1,"write_workspaces":["relative"]}"#,
        br#"{"schema_version":1,"write_workspaces":true}"#,
        b"not-json private-test-value",
        &[0xff],
    ] {
        let error = Settings::from_json(bytes).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(!format!("{error:?}").contains("private-test-value"));
    }
    assert!(Settings::from_json(&vec![b' '; 65537]).is_err());
    let excessive =
        serde_json::json!({"schema_version":1,"write_workspaces":vec![std::env::temp_dir();257]});
    assert!(Settings::from_json(&serde_json::to_vec(&excessive).unwrap()).is_err());
}

#[test]
fn saved_configuration_is_bounded_and_never_replaced_by_defaults_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    let store = ConfigStore::new(paths.clone());
    store.initialize().unwrap();
    for bytes in [b"invalid".to_vec(), vec![b' '; 65537]] {
        fs::write(paths.config_file(), &bytes).unwrap();
        assert!(store.load().is_err());
        assert_eq!(fs::read(paths.config_file()).unwrap(), bytes);
    }
}

#[test]
fn valid_profiles_and_exact_workspace_grants_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("child")).unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    let child = Workspace::open(&dir.path().join("child")).unwrap();
    let input = serde_json::json!({"schema_version":1,"preferred_local_profile":"dociler-pro","write_workspaces":[workspace.root()]});
    let settings = Settings::from_json(&serde_json::to_vec(&input).unwrap()).unwrap();
    assert_eq!(settings.preferred_local_profile(), LocalProfile::Pro);
    assert_eq!(
        settings.write_policy(&workspace),
        WritePolicy::ConfirmEveryWrite
    );
    assert_eq!(settings.write_policy(&child), WritePolicy::ReadOnly);
    let roundtrip = Settings::from_json(&serde_json::to_vec(&settings).unwrap()).unwrap();
    assert_eq!(
        roundtrip.write_policy(&workspace),
        WritePolicy::ConfirmEveryWrite
    );
}

#[test]
fn history_is_bounded_clearable_and_never_written_to_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(dir.path()).unwrap();
    let mut session = Session::new(workspace.clone());
    assert_eq!(session.workspace(), &workspace);
    session
        .push(Role::User, "Ringkas dokumen — 日本語".into())
        .unwrap();
    assert_eq!(session.messages()[0].role(), Role::User);
    assert_eq!(session.messages()[0].text(), "Ringkas dokumen — 日本語");
    assert!(
        session
            .push(Role::Assistant, "x".repeat(1024 * 1024))
            .is_err()
    );
    assert_eq!(session.messages().len(), 1);
    session.clear();
    assert!(session.messages().is_empty());
    session
        .push(Role::Assistant, "x".repeat(1024 * 1024))
        .unwrap();
    assert!(session.push(Role::User, "x".into()).is_err());
    session.clear();
    for _ in 0..512 {
        session.push(Role::User, String::new()).unwrap();
    }
    assert!(session.push(Role::User, String::new()).is_err());
    drop(session);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(Session::new(workspace).messages().is_empty());
}

#[test]
fn credentials_are_redacted_and_unavailable_store_never_falls_back() {
    let secret = Secret::new("private-test-value".into());
    assert_eq!(secret.expose(), "private-test-value");
    assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
    let id = CredentialId::new("upstream_1").unwrap();
    assert_eq!(id.as_str(), "upstream_1");
    for bad in ["", "../path", "https://example.test", "newline\n"] {
        assert_eq!(
            CredentialId::new(bad).unwrap_err(),
            CredentialError::InvalidId
        );
    }
    assert!(CredentialId::new(&"x".repeat(65)).is_err());
    let store = UnavailableCredentialStore;
    assert_eq!(store.get(&id).unwrap_err(), CredentialError::Unavailable);
    assert_eq!(store.set(&id, &secret), Err(CredentialError::Unavailable));
    assert_eq!(store.delete(&id), Err(CredentialError::Unavailable));
}

#[cfg(unix)]
#[test]
fn symlinked_settings_and_overly_open_permissions_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    let store = ConfigStore::new(paths.clone());
    store.initialize().unwrap();
    let file = paths.config_file();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.load().is_err());
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&paths.config_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(store.load().is_err());
    fs::set_permissions(&paths.config_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let moved = dir.path().join("original.json");
    fs::rename(&file, &moved).unwrap();
    symlink(&moved, &file).unwrap();
    assert!(store.load().is_err());
    assert!(store.initialize().is_err());
    assert!(Settings::from_json(&fs::read(&moved).unwrap()).is_ok());
    let moved_dir = dir.path().join("original-config");
    fs::rename(&paths.config_dir, &moved_dir).unwrap();
    symlink(&moved_dir, &paths.config_dir).unwrap();
    assert!(store.load().is_err());
    assert!(store.initialize().is_err());
}

#[cfg(unix)]
#[test]
fn workspace_aliases_cannot_retarget_a_canonical_grant() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    let other = dir.path().join("other");
    let alias = dir.path().join("alias");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&other).unwrap();
    symlink(&original, &alias).unwrap();
    let workspace = Workspace::open(&alias).unwrap();
    assert_eq!(workspace, Workspace::open(&original).unwrap());
    let input = serde_json::json!({"schema_version":1,"write_workspaces":[workspace.root()]});
    let settings = Settings::from_json(&serde_json::to_vec(&input).unwrap()).unwrap();
    assert_eq!(
        settings.write_policy(&workspace),
        WritePolicy::ConfirmEveryWrite
    );
    fs::remove_file(&alias).unwrap();
    symlink(&other, &alias).unwrap();
    assert_eq!(
        settings.write_policy(&Workspace::open(&alias).unwrap()),
        WritePolicy::ReadOnly
    );
}
