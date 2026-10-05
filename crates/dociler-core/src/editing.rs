//! In-place document editing with diff preview, confirmation gates, and memory-only undo.
//!
//! Enforces:
//! - Only Markdown (`.md`) and PlainText (`.txt`) can be edited in place.
//! - In-place editing of binary or legacy formats (PDF, DOC, DOCX, RTF, ODT) is hard-denied.
//! - Path containment within canonical workspace boundaries.
//! - Write grant policy check (read-only workspaces cannot be modified).
//! - Explicit confirmation requirement before any file mutation.
//! - Atomic file replacement via same-directory temporary file and sync.
//! - In-memory session undo stack that drops pre-edit bytes on exit.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::document::DocumentFormat;
use crate::workspace::{DiscoveryError, Workspace, WritePolicy};

/// Errors occurring during document editing or undo operations.
#[derive(Debug)]
pub enum EditError {
    /// Workspace write grant required; workspace is currently read-only.
    PermissionDenied { workspace_root: PathBuf },
    /// In-place write requires explicit user confirmation.
    ConfirmationRequired { path: PathBuf },
    /// Binary and legacy formats (PDF, DOC, DOCX, RTF, ODT) cannot be edited in place.
    BinaryOverwriteDenied {
        format: DocumentFormat,
        path: PathBuf,
    },
    /// Target path escapes the workspace root boundary.
    PathEscape { path: PathBuf },
    /// No undo history available for the requested document.
    NoUndoAvailable { path: PathBuf },
    /// Standard I/O failure.
    Io(io::Error),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionDenied { workspace_root } => write!(
                f,
                "workspace '{}' is read-only; write permission required",
                workspace_root.display()
            ),
            Self::ConfirmationRequired { path } => write!(
                f,
                "in-place edit to '{}' requires explicit confirmation",
                path.display()
            ),
            Self::BinaryOverwriteDenied { format, path } => write!(
                f,
                "refusing in-place edit of {:?} file '{}'; only .md and .txt support direct edits",
                format,
                path.display()
            ),
            Self::PathEscape { path } => write!(
                f,
                "target path '{}' resolves outside workspace boundary",
                path.display()
            ),
            Self::NoUndoAvailable { path } => {
                write!(
                    f,
                    "no in-memory undo point available for '{}'",
                    path.display()
                )
            }
            Self::Io(err) => write!(f, "edit I/O error: {err}"),
        }
    }
}

impl std::error::Error for EditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for EditError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<DiscoveryError> for EditError {
    fn from(err: DiscoveryError) -> Self {
        match err {
            DiscoveryError::PathEscape { symlink, .. } => Self::PathEscape { path: symlink },
            DiscoveryError::Io(err) => Self::Io(err),
            _ => Self::Io(io::Error::new(io::ErrorKind::Other, err.to_string())),
        }
    }
}

/// Unified diff representation for previewing in-place document edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPreview {
    pub path: PathBuf,
    pub diff_text: String,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub lines_unchanged: usize,
}

impl DiffPreview {
    /// Generates a unified diff comparing original text with proposed text.
    pub fn generate(path: &Path, original: &str, proposed: &str) -> Self {
        let orig_lines: Vec<&str> = original.lines().collect();
        let prop_lines: Vec<&str> = proposed.lines().collect();

        let mut diff_lines = Vec::new();
        diff_lines.push(format!("--- a/{}", path.display()));
        diff_lines.push(format!("+++ b/{}", path.display()));

        let mut added = 0;
        let mut removed = 0;
        let mut unchanged = 0;

        let max_len = orig_lines.len().max(prop_lines.len());
        let mut hunk_lines = Vec::new();

        let mut i = 0;
        let mut j = 0;

        while i < orig_lines.len() || j < prop_lines.len() {
            if i < orig_lines.len() && j < prop_lines.len() && orig_lines[i] == prop_lines[j] {
                hunk_lines.push(format!(" {}", orig_lines[i]));
                unchanged += 1;
                i += 1;
                j += 1;
            } else {
                if i < orig_lines.len() {
                    hunk_lines.push(format!("-{}", orig_lines[i]));
                    removed += 1;
                    i += 1;
                }
                if j < prop_lines.len() {
                    hunk_lines.push(format!("+{}", prop_lines[j]));
                    added += 1;
                    j += 1;
                }
            }
        }

        diff_lines.push(format!(
            "@@ -1,{} +1,{} @@",
            orig_lines.len(),
            prop_lines.len()
        ));
        diff_lines.extend(hunk_lines);

        let _ = max_len;

        Self {
            path: path.to_path_buf(),
            diff_text: diff_lines.join("\n"),
            lines_added: added,
            lines_removed: removed,
            lines_unchanged: unchanged,
        }
    }
}

/// In-memory record of a pre-edit file state for session-scoped undo.
#[derive(Debug, Clone)]
struct UndoRecord {
    previous_content: Vec<u8>,
    _timestamp: SystemTime,
}

/// Bounded in-memory session undo stack. Dropped on process exit without disk residue.
#[derive(Debug, Default)]
pub struct SessionUndoStack {
    records: HashMap<PathBuf, Vec<UndoRecord>>,
}

impl SessionUndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records pre-edit bytes for a document path.
    pub fn push(&mut self, path: &Path, previous_content: Vec<u8>) {
        self.records
            .entry(path.to_path_buf())
            .or_default()
            .push(UndoRecord {
                previous_content,
                _timestamp: SystemTime::now(),
            });
    }

    /// Pops the latest pre-edit state for a document path.
    pub fn pop(&mut self, path: &Path) -> Option<Vec<u8>> {
        let stack = self.records.get_mut(path)?;
        let record = stack.pop()?;
        if stack.is_empty() {
            self.records.remove(path);
        }
        Some(record.previous_content)
    }

    /// Returns whether undo is available for the given document path.
    pub fn can_undo(&self, path: &Path) -> bool {
        self.records.get(path).is_some_and(|s| !s.is_empty())
    }

    /// Number of available undo operations for the given path.
    pub fn undo_count(&self, path: &Path) -> usize {
        self.records.get(path).map_or(0, |s| s.len())
    }

    /// Total bytes held in memory across all session undo records.
    pub fn total_bytes(&self) -> usize {
        self.records
            .values()
            .flat_map(|stack| stack.iter().map(|r| r.previous_content.len()))
            .sum()
    }

    /// Clears all stored undo records from session memory.
    pub fn clear(&mut self) {
        self.records.clear();
    }
}

/// Applies an in-place atomic edit to a Markdown or PlainText document with safety guards.
pub fn apply_in_place_edit(
    workspace: &Workspace,
    write_policy: WritePolicy,
    confirmed: bool,
    relative_path: &Path,
    new_content: &str,
    undo_stack: &mut SessionUndoStack,
) -> Result<DiffPreview, EditError> {
    // 1. Verify workspace containment
    let target_path = if relative_path.is_relative() {
        workspace.root().join(relative_path)
    } else {
        relative_path.to_path_buf()
    };

    // If file exists, ensure canonical path is inside workspace
    if target_path.exists() {
        let canonical = target_path.canonicalize().map_err(EditError::Io)?;
        if !canonical.starts_with(workspace.root()) {
            return Err(EditError::PathEscape {
                path: relative_path.to_path_buf(),
            });
        }
    } else {
        // Target parent directory must be inside workspace
        let parent = target_path.parent().unwrap_or_else(|| workspace.root());
        let canonical_parent = parent.canonicalize().map_err(EditError::Io)?;
        if !canonical_parent.starts_with(workspace.root()) {
            return Err(EditError::PathEscape {
                path: relative_path.to_path_buf(),
            });
        }
    }

    // 2. Validate format: only MD and TXT support in-place edits
    let format = DocumentFormat::from_path(&target_path).unwrap_or(DocumentFormat::PlainText);
    if !format.is_direct_editable() {
        return Err(EditError::BinaryOverwriteDenied {
            format,
            path: relative_path.to_path_buf(),
        });
    }

    // 3. Verify write grant
    if write_policy == WritePolicy::ReadOnly {
        return Err(EditError::PermissionDenied {
            workspace_root: workspace.root().to_path_buf(),
        });
    }

    // 4. Verify explicit confirmation
    if !confirmed {
        return Err(EditError::ConfirmationRequired {
            path: relative_path.to_path_buf(),
        });
    }

    // 5. Read existing content for diff and undo
    let original_bytes = if target_path.exists() {
        fs::read(&target_path).map_err(EditError::Io)?
    } else {
        Vec::new()
    };
    let original_text = String::from_utf8_lossy(&original_bytes);
    let diff = DiffPreview::generate(relative_path, &original_text, new_content);

    // 6. Record pre-edit state in session memory
    undo_stack.push(relative_path, original_bytes);

    // 7. Atomic write: write to same-directory temp file, fsync, then rename
    atomic_write_file(&target_path, new_content.as_bytes())?;

    Ok(diff)
}

/// Reverts the latest edit for a document using session-memory undo.
pub fn undo_edit(
    workspace: &Workspace,
    relative_path: &Path,
    undo_stack: &mut SessionUndoStack,
) -> Result<(), EditError> {
    let target_path = if relative_path.is_relative() {
        workspace.root().join(relative_path)
    } else {
        relative_path.to_path_buf()
    };

    let previous_bytes =
        undo_stack
            .pop(relative_path)
            .ok_or_else(|| EditError::NoUndoAvailable {
                path: relative_path.to_path_buf(),
            })?;

    if previous_bytes.is_empty() && !target_path.exists() {
        return Ok(());
    }

    atomic_write_file(&target_path, &previous_bytes)?;
    Ok(())
}

/// Atomically writes content to `target_path` using a temporary file in the same directory.
fn atomic_write_file(target_path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let temp_name = format!(
        ".tmp.{}.{}",
        target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("doc"),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp_path = parent.join(temp_name);

    let mut file = File::create(&temp_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600));
    }

    file.write_all(content)?;
    file.sync_all()?;
    drop(file);

    fs::rename(&temp_path, target_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_preview_detects_additions_and_removals() {
        let orig = "Line 1\nLine 2\nLine 3";
        let prop = "Line 1\nLine 2 modified\nLine 3\nLine 4";
        let diff = DiffPreview::generate(Path::new("test.md"), orig, prop);

        assert_eq!(diff.lines_unchanged, 2); // Line 1 and Line 3
        assert_eq!(diff.lines_added, 2); // modified line 2 and line 4
        assert_eq!(diff.lines_removed, 1); // original line 2
        assert!(diff.diff_text.contains("--- a/test.md"));
        assert!(diff.diff_text.contains("+++ b/test.md"));
        assert!(diff.diff_text.contains("+Line 4"));
    }

    #[test]
    fn in_place_edit_refuses_binary_formats() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(temp_dir.path()).unwrap();
        let mut undo = SessionUndoStack::new();

        let binary_path = Path::new("report.docx");
        let res = apply_in_place_edit(
            &ws,
            WritePolicy::ConfirmEveryWrite,
            true,
            binary_path,
            "new text",
            &mut undo,
        );

        assert!(matches!(res, Err(EditError::BinaryOverwriteDenied { .. })));
    }

    #[test]
    fn in_place_edit_requires_write_policy_grant() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(temp_dir.path()).unwrap();
        let mut undo = SessionUndoStack::new();

        let res = apply_in_place_edit(
            &ws,
            WritePolicy::ReadOnly,
            true,
            Path::new("doc.md"),
            "new text",
            &mut undo,
        );

        assert!(matches!(res, Err(EditError::PermissionDenied { .. })));
    }

    #[test]
    fn in_place_edit_requires_confirmation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(temp_dir.path()).unwrap();
        let mut undo = SessionUndoStack::new();

        let res = apply_in_place_edit(
            &ws,
            WritePolicy::ConfirmEveryWrite,
            false, // Not confirmed
            Path::new("doc.md"),
            "new text",
            &mut undo,
        );

        assert!(matches!(res, Err(EditError::ConfirmationRequired { .. })));
    }

    #[test]
    fn in_place_edit_and_session_undo() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("notes.txt");
        fs::write(&file_path, "Initial draft content").unwrap();

        let ws = Workspace::open(temp_dir.path()).unwrap();
        let mut undo = SessionUndoStack::new();

        // 1. Apply edit
        let diff = apply_in_place_edit(
            &ws,
            WritePolicy::ConfirmEveryWrite,
            true,
            Path::new("notes.txt"),
            "Updated polished content",
            &mut undo,
        )
        .unwrap();

        assert_eq!(diff.lines_added, 1);
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "Updated polished content"
        );
        assert!(undo.can_undo(Path::new("notes.txt")));

        // 2. Perform undo
        undo_edit(&ws, Path::new("notes.txt"), &mut undo).unwrap();
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "Initial draft content"
        );
        assert!(!undo.can_undo(Path::new("notes.txt")));
    }
}
