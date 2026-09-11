//! Bounded, versioned, non-secret configuration with fail-closed loading.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths::AppPaths;
use crate::workspace::{Workspace, WritePolicy};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalProfile {
    #[default]
    #[serde(rename = "dociler-lite")]
    Lite,
    #[serde(rename = "dociler-pro")]
    Pro,
}

impl LocalProfile {
    pub fn alias(self) -> &'static str {
        match self {
            Self::Lite => "dociler-lite",
            Self::Pro => "dociler-pro",
        }
    }
}

/// No credentials, endpoints, prompts, transcripts, or extracted content belong here.
/// Fields are private so callers cannot bypass validation when constructing settings.
#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    schema_version: u32,
    preferred_local_profile: LocalProfile,
    write_workspaces: Vec<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSettings {
    schema_version: u32,
    #[serde(default)]
    preferred_local_profile: LocalProfile,
    #[serde(default)]
    write_workspaces: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            preferred_local_profile: LocalProfile::Lite,
            write_workspaces: Vec::new(),
        }
    }
}

impl Settings {
    pub fn from_json(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(invalid_config());
        }
        // Never expose serde errors: they may include a secret inserted into a bad field.
        let settings: RawSettings = serde_json::from_slice(bytes).map_err(|_| invalid_config())?;
        if settings.schema_version != 1 || settings.write_workspaces.len() > 256 {
            return Err(invalid_config());
        }
        for path in &settings.write_workspaces {
            if !path.is_absolute() {
                return Err(invalid_config());
            }
        }
        Ok(Self {
            schema_version: settings.schema_version,
            preferred_local_profile: settings.preferred_local_profile,
            write_workspaces: settings.write_workspaces,
        })
    }

    pub fn preferred_local_profile(&self) -> LocalProfile {
        self.preferred_local_profile
    }

    pub fn write_policy(&self, workspace: &Workspace) -> WritePolicy {
        // Exact canonical identity only; no parent/child inheritance and no
        // re-canonicalization of stored aliases that could retarget an old grant.
        if self
            .write_workspaces
            .iter()
            .any(|root| root == workspace.root())
        {
            WritePolicy::ConfirmEveryWrite
        } else {
            WritePolicy::ReadOnly
        }
    }
}

fn invalid_config() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or unsupported configuration",
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Defaults,
    Saved,
}

pub struct LoadedSettings {
    pub settings: Settings,
    pub source: ConfigSource,
}

pub struct ConfigStore {
    paths: AppPaths,
}

impl ConfigStore {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    /// Missing settings are defaults, not a reason to create a file.
    pub fn load(&self) -> io::Result<LoadedSettings> {
        match fs::symlink_metadata(&self.paths.config_dir) {
            Ok(metadata) => check_private(&metadata, true)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(defaults()),
            Err(error) => return Err(error),
        }
        let metadata = match fs::symlink_metadata(self.paths.config_file()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(defaults()),
            Err(error) => return Err(error),
        };
        check_private(&metadata, false)?;
        if metadata.len() > MAX_CONFIG_BYTES {
            return Err(invalid_config());
        }
        let file = File::open(self.paths.config_file())?;
        check_private(&file.metadata()?, false)?;
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
        Ok(LoadedSettings {
            settings: Settings::from_json(&bytes)?,
            source: ConfigSource::Saved,
        })
    }

    /// Explicitly initialize defaults, atomically and without replacing user work.
    /// Updating settings/grants will be added with the permissions UI.
    pub fn initialize(&self) -> io::Result<()> {
        let bytes =
            serde_json::to_vec_pretty(&Settings::default()).map_err(|_| invalid_config())?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.paths.config_dir)?;
        check_private(&fs::symlink_metadata(&self.paths.config_dir)?, true)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.paths.config_dir)?;
        // NamedTempFile creates mode 0600 on Unix; validate before writing.
        check_private(&temporary.as_file().metadata()?, false)?;
        temporary.write_all(&bytes)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(self.paths.config_file())
            .map_err(|error| error.error)?;
        Ok(())
    }
}

fn defaults() -> LoadedSettings {
    LoadedSettings {
        settings: Settings::default(),
        source: ConfigSource::Defaults,
    }
}

fn check_private(metadata: &fs::Metadata, directory: bool) -> io::Result<()> {
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(invalid_config());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "configuration must be user-only",
            ));
        }
    }
    Ok(())
}
