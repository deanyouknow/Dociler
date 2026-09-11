//! Read-only diagnostics. Inspecting a workspace never enumerates its contents.

use std::io;
use std::path::{Path, PathBuf};

/// Facts about the executable's platform and a caller-selected directory.
#[derive(Debug)]
pub struct Diagnostics {
    pub workspace: PathBuf,
    pub operating_system: &'static str,
    pub architecture: &'static str,
}

impl Diagnostics {
    /// Canonicalize a directory without reading files or creating local state.
    ///
    /// This is not a file-access permission grant or an inference readiness test.
    pub fn inspect(workspace: &Path) -> io::Result<Self> {
        let workspace = workspace.canonicalize()?;
        if !workspace.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace must be a directory",
            ));
        }

        Ok(Self {
            workspace,
            operating_system: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
        })
    }
}
