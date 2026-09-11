//! OS-managed locations, resolved without creating any directories.

use std::io;
use std::path::PathBuf;

use directories::ProjectDirs;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> io::Result<Self> {
        let dirs = ProjectDirs::from("", "", "dociler")
            .ok_or_else(|| io::Error::other("cannot locate user directories"))?;
        Self::new(
            std::env::var_os("DOCILER_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| dirs.config_local_dir().to_owned()),
            dirs.data_local_dir().to_owned(),
            dirs.cache_dir().to_owned(),
        )
    }

    /// Injectable paths for embedders/tests, not a workspace config override.
    pub fn new(config_dir: PathBuf, data_dir: PathBuf, cache_dir: PathBuf) -> io::Result<Self> {
        if [&config_dir, &data_dir, &cache_dir]
            .iter()
            .any(|path| !path.is_absolute())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "application directories must be absolute",
            ));
        }
        Ok(Self {
            config_dir,
            data_dir,
            cache_dir,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    pub fn runtimes_dir(&self) -> PathBuf {
        self.data_dir.join("runtimes")
    }
}
