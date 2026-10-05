//! Canonical workspace identity, gitignore-aware document discovery, and write policy.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::document::DocumentFormat;

/// Canonical workspace root directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Opens and validates a workspace root directory, ensuring canonical resolution.
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

    /// The canonical filesystem root for this workspace.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Verifies that a target path is strictly contained within the workspace root.
    ///
    /// Resolves symlinks and relative references. Rejects attempts to escape
    /// the workspace boundary via path traversal or symlinks.
    pub fn canonical_containment(&self, path: &Path) -> Result<PathBuf, DiscoveryError> {
        let resolved = if path.is_relative() {
            self.root.join(path)
        } else {
            path.to_path_buf()
        };
        let canonical = resolved.canonicalize().map_err(DiscoveryError::Io)?;
        if !canonical.starts_with(&self.root) {
            return Err(DiscoveryError::PathEscape {
                symlink: path.to_path_buf(),
                target: canonical,
            });
        }
        Ok(canonical)
    }

    /// Discovers supported word-processing and text documents in the workspace using default options.
    pub fn discover_documents(&self) -> Result<DiscoveryReport, DiscoveryError> {
        self.discover_documents_with_options(&DiscoveryOptions::default())
    }

    /// Discovers supported documents in the workspace using configured discovery options.
    pub fn discover_documents_with_options(
        &self,
        options: &DiscoveryOptions,
    ) -> Result<DiscoveryReport, DiscoveryError> {
        let mut report = DiscoveryReport::default();
        let mut gitignore_stack = Vec::new();

        walk_workspace_dir(
            &self.root,
            &self.root,
            options,
            &mut gitignore_stack,
            &mut report,
        )?;

        // Ensure deterministic ordering across platforms and filesystems
        report
            .documents
            .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        Ok(report)
    }
}

/// A persisted grant never authorizes an individual mutation on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    ReadOnly,
    ConfirmEveryWrite,
}

/// Metadata for a discovered document within a workspace.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredDocument {
    /// Relative path from workspace root, suitable for display and user referencing.
    pub relative_path: PathBuf,
    /// Absolute canonical path on the filesystem.
    pub canonical_path: PathBuf,
    /// Recognized document format.
    pub format: DocumentFormat,
    /// File size in bytes.
    pub byte_size: u64,
}

impl DiscoveredDocument {
    /// Whether this document format supports atomic in-place edits.
    pub fn is_direct_editable(&self) -> bool {
        self.format.is_direct_editable()
    }

    /// Whether this document format can be generated as a new export.
    pub fn supports_export(&self) -> bool {
        self.format.supports_export()
    }

    /// The primary file extension for this document.
    pub fn primary_extension(&self) -> &'static str {
        self.format.primary_extension()
    }
}

/// Configuration options for workspace document discovery.
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Maximum permitted byte size for an individual document (default: 50 MiB).
    pub max_file_size_bytes: u64,
    /// Maximum aggregate byte size across all discovered documents (default: 250 MiB).
    pub max_total_bytes: u64,
    /// Maximum number of discovered files before failing closed (default: 10,000).
    pub max_files: usize,
    /// Whether to parse and respect `.gitignore` rules (default: true).
    pub respect_gitignore: bool,
    /// Whether to exclude hidden files and directories starting with `.` (default: true).
    pub exclude_hidden: bool,
    /// Directory names excluded from traversal by default.
    pub excluded_dir_names: Vec<String>,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            max_file_size_bytes: 50 * 1024 * 1024, // 50 MiB
            max_total_bytes: 250 * 1024 * 1024,    // 250 MiB
            max_files: 10_000,
            respect_gitignore: true,
            exclude_hidden: true,
            excluded_dir_names: vec![
                ".git".to_string(),
                ".github".to_string(),
                "target".to_string(),
                "node_modules".to_string(),
                "dist".to_string(),
                "build".to_string(),
                ".cache".to_string(),
                "vendor".to_string(),
                ".cargo".to_string(),
                ".gemini".to_string(),
                ".vscode".to_string(),
                ".idea".to_string(),
                "models".to_string(),
            ],
        }
    }
}

/// Summary report of workspace document discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// Discovered supported documents, deterministically sorted by relative path.
    pub documents: Vec<DiscoveredDocument>,
    /// Number of files ignored due to `.gitignore` or exclusion rules.
    pub ignored_files_count: usize,
    /// Number of symlinks skipped (directory symlinks or symlinks escaping the workspace).
    pub skipped_symlinks_count: usize,
    /// Number of supported document files skipped because they exceeded `max_file_size_bytes`.
    pub skipped_oversized_count: usize,
    /// Total aggregate byte size of all discovered documents.
    pub total_document_bytes: u64,
}

impl DiscoveryReport {
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }
}

/// Errors occurring during workspace document discovery.
#[derive(Debug)]
pub enum DiscoveryError {
    Io(io::Error),
    TotalSizeLimitExceeded { limit: u64, found: u64 },
    FileLimitExceeded { limit: usize },
    PathEscape { symlink: PathBuf, target: PathBuf },
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "workspace I/O error: {err}"),
            Self::TotalSizeLimitExceeded { limit, found } => {
                write!(
                    f,
                    "workspace documents total size ({found} bytes) exceeded ceiling ({limit} bytes)"
                )
            }
            Self::FileLimitExceeded { limit } => {
                write!(
                    f,
                    "workspace document count exceeded limit of {limit} files"
                )
            }
            Self::PathEscape { symlink, target } => {
                write!(
                    f,
                    "symlink '{}' escapes workspace boundary (points to '{}')",
                    symlink.display(),
                    target.display()
                )
            }
        }
    }
}

impl std::error::Error for DiscoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for DiscoveryError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Recursive directory walker for workspace documents.
fn walk_workspace_dir(
    current_dir: &Path,
    workspace_root: &Path,
    options: &DiscoveryOptions,
    gitignore_stack: &mut Vec<GitIgnoreFile>,
    report: &mut DiscoveryReport,
) -> Result<(), DiscoveryError> {
    let mut pushed_gitignore = false;
    if options.respect_gitignore {
        let gitignore_path = current_dir.join(".gitignore");
        if gitignore_path.is_file() {
            if let Ok(content) = fs::read_to_string(&gitignore_path) {
                let file = GitIgnoreFile::parse(current_dir.to_path_buf(), &content);
                gitignore_stack.push(file);
                pushed_gitignore = true;
            }
        }
    }

    let read_dir = match fs::read_dir(current_dir) {
        Ok(rd) => rd,
        Err(err) => {
            if pushed_gitignore {
                gitignore_stack.pop();
            }
            return Err(DiscoveryError::Io(err));
        }
    };

    let mut entries = Vec::new();
    for entry in read_dir.flatten() {
        entries.push(entry);
    }
    // Sort entries deterministically by file name
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let file_name_str = match file_name.to_str() {
            Some(s) => s,
            None => continue,
        };

        // Hidden files/directories starting with '.'
        if options.exclude_hidden && file_name_str.starts_with('.') {
            continue;
        }

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_symlink() {
            // Inspect symlink target
            let target_meta = match fs::metadata(&entry_path) {
                Ok(m) => m,
                Err(_) => {
                    // Broken symlink
                    report.skipped_symlinks_count += 1;
                    continue;
                }
            };

            // "Do not follow directory symlinks."
            if target_meta.is_dir() {
                report.skipped_symlinks_count += 1;
                continue;
            }

            // Symlink to file: verify path containment
            let canonical_target = match entry_path.canonicalize() {
                Ok(p) => p,
                Err(_) => {
                    report.skipped_symlinks_count += 1;
                    continue;
                }
            };

            if !canonical_target.starts_with(workspace_root) {
                // Escaping symlink
                report.skipped_symlinks_count += 1;
                continue;
            }

            // Target is a file contained within workspace
            let rel_path = match entry_path.strip_prefix(workspace_root) {
                Ok(p) => p,
                Err(_) => continue,
            };

            if options.respect_gitignore && is_path_ignored(gitignore_stack, &entry_path, false) {
                report.ignored_files_count += 1;
                continue;
            }

            if let Some(format) = DocumentFormat::from_path(&entry_path) {
                let byte_size = target_meta.len();
                if byte_size > options.max_file_size_bytes {
                    report.skipped_oversized_count += 1;
                    continue;
                }
                if report.total_document_bytes + byte_size > options.max_total_bytes {
                    if pushed_gitignore {
                        gitignore_stack.pop();
                    }
                    return Err(DiscoveryError::TotalSizeLimitExceeded {
                        limit: options.max_total_bytes,
                        found: report.total_document_bytes + byte_size,
                    });
                }
                if report.documents.len() >= options.max_files {
                    if pushed_gitignore {
                        gitignore_stack.pop();
                    }
                    return Err(DiscoveryError::FileLimitExceeded {
                        limit: options.max_files,
                    });
                }
                report.total_document_bytes += byte_size;
                report.documents.push(DiscoveredDocument {
                    relative_path: rel_path.to_path_buf(),
                    canonical_path: canonical_target,
                    format,
                    byte_size,
                });
            }
            continue;
        }

        if file_type.is_dir() {
            // Exclude default build/vendor/cache directory names
            if options
                .excluded_dir_names
                .iter()
                .any(|d| d == file_name_str)
            {
                continue;
            }

            if options.respect_gitignore && is_path_ignored(gitignore_stack, &entry_path, true) {
                continue;
            }

            walk_workspace_dir(
                &entry_path,
                workspace_root,
                options,
                gitignore_stack,
                report,
            )?;
            continue;
        }

        if file_type.is_file() {
            let rel_path = match entry_path.strip_prefix(workspace_root) {
                Ok(p) => p,
                Err(_) => continue,
            };

            if options.respect_gitignore && is_path_ignored(gitignore_stack, &entry_path, false) {
                report.ignored_files_count += 1;
                continue;
            }

            if let Some(format) = DocumentFormat::from_path(&entry_path) {
                let byte_size = match entry.metadata() {
                    Ok(m) => m.len(),
                    Err(_) => continue,
                };
                if byte_size > options.max_file_size_bytes {
                    report.skipped_oversized_count += 1;
                    continue;
                }
                if report.total_document_bytes + byte_size > options.max_total_bytes {
                    if pushed_gitignore {
                        gitignore_stack.pop();
                    }
                    return Err(DiscoveryError::TotalSizeLimitExceeded {
                        limit: options.max_total_bytes,
                        found: report.total_document_bytes + byte_size,
                    });
                }
                if report.documents.len() >= options.max_files {
                    if pushed_gitignore {
                        gitignore_stack.pop();
                    }
                    return Err(DiscoveryError::FileLimitExceeded {
                        limit: options.max_files,
                    });
                }
                let canonical_path = match entry_path.canonicalize() {
                    Ok(p) => p,
                    Err(_) => entry_path.clone(),
                };
                report.total_document_bytes += byte_size;
                report.documents.push(DiscoveredDocument {
                    relative_path: rel_path.to_path_buf(),
                    canonical_path,
                    format,
                    byte_size,
                });
            }
        }
    }

    if pushed_gitignore {
        gitignore_stack.pop();
    }

    Ok(())
}

/// Evaluates gitignore status against a stack of `.gitignore` files from innermost to root.
fn is_path_ignored(gitignore_stack: &[GitIgnoreFile], full_path: &Path, is_dir: bool) -> bool {
    for gitignore in gitignore_stack.iter().rev() {
        if let Ok(rel) = full_path.strip_prefix(&gitignore.base_dir) {
            if let Some(decision) = gitignore.matches(rel, is_dir) {
                return decision;
            }
        }
    }
    false
}

#[derive(Debug, Clone)]
struct GitIgnoreFile {
    base_dir: PathBuf,
    rules: Vec<GitIgnoreRule>,
}

impl GitIgnoreFile {
    fn parse(base_dir: PathBuf, content: &str) -> Self {
        let mut rules = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(rule) = GitIgnoreRule::parse(trimmed) {
                rules.push(rule);
            }
        }
        Self { base_dir, rules }
    }

    fn matches(&self, rel_path: &Path, is_dir: bool) -> Option<bool> {
        let mut last_match = None;
        for rule in &self.rules {
            if rule.matches(rel_path, is_dir) {
                last_match = Some(!rule.negated);
            }
        }
        last_match
    }
}

#[derive(Debug, Clone)]
struct GitIgnoreRule {
    pattern_parts: Vec<String>,
    negated: bool,
    dir_only: bool,
    anchored: bool,
}

impl GitIgnoreRule {
    fn parse(raw: &str) -> Option<Self> {
        let mut s = raw.trim();
        let negated = s.starts_with('!');
        if negated {
            s = s[1..].trim();
        }
        let dir_only = s.ends_with('/');
        if dir_only {
            s = s.trim_end_matches('/');
        }
        if s.is_empty() {
            return None;
        }
        let anchored = if s.starts_with('/') {
            s = &s[1..];
            true
        } else {
            s.contains('/')
        };

        let pattern_parts = s.split('/').map(String::from).collect();
        Some(Self {
            pattern_parts,
            negated,
            dir_only,
            anchored,
        })
    }

    fn matches(&self, rel_path: &Path, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }

        let path_parts: Vec<&str> = rel_path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        if path_parts.is_empty() {
            return false;
        }

        if self.anchored {
            match_path_components(&self.pattern_parts, &path_parts)
        } else {
            // Unanchored pattern (e.g. `*.md` or `target`)
            let pattern_str = &self.pattern_parts[0];
            if is_dir {
                path_parts
                    .iter()
                    .any(|comp| matches_glob(pattern_str, comp))
            } else {
                path_parts
                    .last()
                    .is_some_and(|last| matches_glob(pattern_str, last))
            }
        }
    }
}

fn match_path_components(pattern: &[String], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        for i in 0..=path.len() {
            if match_path_components(&pattern[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }
    if path.is_empty() {
        return false;
    }
    if matches_glob(&pattern[0], path[0]) {
        return match_path_components(&pattern[1..], &path[1..]);
    }
    false
}

fn matches_glob(pattern: &str, text: &str) -> bool {
    let p_bytes = pattern.as_bytes();
    let t_bytes = text.as_bytes();
    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut star_p = None;
    let mut star_t = 0;

    while t_idx < t_bytes.len() {
        if p_idx < p_bytes.len() && (p_bytes[p_idx] == b'?' || p_bytes[p_idx] == t_bytes[t_idx]) {
            p_idx += 1;
            t_idx += 1;
        } else if p_idx < p_bytes.len() && p_bytes[p_idx] == b'*' {
            star_p = Some(p_idx);
            p_idx += 1;
            star_t = t_idx;
        } else if let Some(sp) = star_p {
            p_idx = sp + 1;
            star_t += 1;
            t_idx = star_t;
        } else {
            return false;
        }
    }
    while p_idx < p_bytes.len() && p_bytes[p_idx] == b'*' {
        p_idx += 1;
    }
    p_idx == p_bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_supported_documents_in_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();

        fs::write(dir.path().join("readme.md"), "# Hello").unwrap();
        fs::write(dir.path().join("notes.txt"), "some notes").unwrap();
        fs::write(dir.path().join("doc.docx"), "mock docx").unwrap();
        fs::write(dir.path().join("binary.exe"), "executable").unwrap();
        fs::write(dir.path().join("image.png"), "png image").unwrap();

        let sub = dir.path().join("docs");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("architecture.md"), "## Arch").unwrap();

        let report = ws.discover_documents().unwrap();
        assert_eq!(report.documents.len(), 4);
        assert_eq!(report.ignored_files_count, 0);

        let names: Vec<_> = report
            .documents
            .iter()
            .map(|d| d.relative_path.to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["doc.docx", "docs/architecture.md", "notes.txt", "readme.md"]
        );

        assert_eq!(report.documents[0].format, DocumentFormat::Docx);
        assert_eq!(report.documents[1].format, DocumentFormat::Markdown);
        assert_eq!(report.documents[2].format, DocumentFormat::PlainText);
        assert_eq!(report.documents[3].format, DocumentFormat::Markdown);
    }

    #[test]
    fn respects_gitignore_rules_and_negations() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();

        let gitignore = r#"
# Comments
target/
*.tmp.md
/secret.md
staging/*
!staging/keep.md
"#;
        fs::write(dir.path().join(".gitignore"), gitignore).unwrap();
        fs::write(dir.path().join("normal.md"), "normal").unwrap();
        fs::write(dir.path().join("draft.tmp.md"), "temporary").unwrap();
        fs::write(dir.path().join("secret.md"), "secret").unwrap();

        let staging_dir = dir.path().join("staging");
        fs::create_dir(&staging_dir).unwrap();
        fs::write(staging_dir.join("drop.md"), "drop").unwrap();
        fs::write(staging_dir.join("keep.md"), "keep").unwrap();

        let report = ws.discover_documents().unwrap();
        let names: Vec<_> = report
            .documents
            .iter()
            .map(|d| d.relative_path.to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["normal.md", "staging/keep.md"]);
        assert!(report.ignored_files_count >= 2);
    }

    #[test]
    fn excludes_default_directories() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();

        let target_dir = dir.path().join("target");
        fs::create_dir(&target_dir).unwrap();
        fs::write(target_dir.join("build_output.md"), "compiled").unwrap();

        let node_modules = dir.path().join("node_modules");
        fs::create_dir(&node_modules).unwrap();
        fs::write(node_modules.join("package.md"), "package").unwrap();

        fs::write(dir.path().join("root.md"), "root doc").unwrap();

        let report = ws.discover_documents().unwrap();
        assert_eq!(report.documents.len(), 1);
        assert_eq!(
            report.documents[0].relative_path.to_str().unwrap(),
            "root.md"
        );
    }

    #[test]
    fn refuses_directory_symlinks_and_escaping_file_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();

        let outside_dir = tempfile::tempdir().unwrap();
        let outside_file = outside_dir.path().join("external.md");
        fs::write(&outside_file, "external content").unwrap();

        // 1. Escaping file symlink: points to outside_file
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside_file, dir.path().join("symlink_outside.md"))
                .unwrap();
        }

        // 2. Directory symlink inside workspace
        let real_sub = dir.path().join("real_sub");
        fs::create_dir(&real_sub).unwrap();
        fs::write(real_sub.join("sub_doc.md"), "sub doc").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_sub, dir.path().join("symlink_dir")).unwrap();
        }

        // 3. Valid in-workspace file symlink
        let internal_file = dir.path().join("internal.md");
        fs::write(&internal_file, "internal content").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&internal_file, dir.path().join("symlink_internal.md"))
                .unwrap();
        }

        let report = ws.discover_documents().unwrap();
        #[cfg(unix)]
        {
            // Directory symlink and escaping symlink must be skipped
            assert!(report.skipped_symlinks_count >= 2);
            let names: Vec<_> = report
                .documents
                .iter()
                .map(|d| d.relative_path.to_str().unwrap())
                .collect();
            assert!(names.contains(&"internal.md"));
            assert!(names.contains(&"real_sub/sub_doc.md"));
            assert!(names.contains(&"symlink_internal.md"));
            assert!(!names.contains(&"symlink_outside.md"));
            assert!(!names.contains(&"symlink_dir/sub_doc.md"));
        }
    }

    #[test]
    fn enforces_file_size_and_total_size_limits() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();

        fs::write(dir.path().join("small.md"), "small").unwrap();
        fs::write(dir.path().join("large.md"), vec![b'a'; 1000]).unwrap();

        // Max file size limit: 500 bytes
        let opts = DiscoveryOptions {
            max_file_size_bytes: 500,
            ..DiscoveryOptions::default()
        };
        let report = ws.discover_documents_with_options(&opts).unwrap();
        assert_eq!(report.documents.len(), 1);
        assert_eq!(
            report.documents[0].relative_path.to_str().unwrap(),
            "small.md"
        );
        assert_eq!(report.skipped_oversized_count, 1);

        // Max total bytes limit: 10 bytes -> triggers error on large file
        let tight_opts = DiscoveryOptions {
            max_file_size_bytes: 5000,
            max_total_bytes: 10,
            ..DiscoveryOptions::default()
        };
        let err = ws.discover_documents_with_options(&tight_opts).unwrap_err();
        assert!(matches!(err, DiscoveryError::TotalSizeLimitExceeded { .. }));
    }

    #[test]
    fn canonical_containment_verifies_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();

        let inside = dir.path().join("inside.md");
        fs::write(&inside, "content").unwrap();

        let canonical = ws.canonical_containment(Path::new("inside.md")).unwrap();
        assert_eq!(canonical, inside.canonicalize().unwrap());

        // Escape attempt
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("other.md");
        fs::write(&outside_file, "other").unwrap();

        let err = ws.canonical_containment(&outside_file).unwrap_err();
        assert!(matches!(err, DiscoveryError::PathEscape { .. }));
    }
}
