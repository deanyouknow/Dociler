//! Canonical workspace identity. This is not yet a document-access sandbox.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn open(path: &Path) -> io::Result<Self> {
        let root = path.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace must be a directory",
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// A persisted grant never authorizes an individual mutation on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    ReadOnly,
    ConfirmEveryWrite,
}
