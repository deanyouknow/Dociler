//! Canonical document AST, supported formats, and native parsing.
//!
//! Dociler normalizes all extracted word-processing formats into an internal
//! abstract syntax tree (AST). Formats and signatures are validated before
//! parsing, and documents never contain executable content, layout coordinates,
//! or external references.

use std::fmt;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Supported word-processing and plain text document formats.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum DocumentFormat {
    /// Markdown (`.md`)
    Markdown,
    /// Plain UTF-8 or ASCII text (`.txt`)
    PlainText,
    /// Portable Document Format (`.pdf`, text extraction only)
    Pdf,
    /// Microsoft Word OpenXML (`.docx`)
    Docx,
    /// Microsoft Word 97-2003 binary format (`.doc`, legacy input only)
    Doc,
    /// Rich Text Format (`.rtf`)
    Rtf,
    /// OpenDocument Text (`.odt`)
    Odt,
}

impl DocumentFormat {
    /// Maps a file extension to its corresponding format.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let normalized = extension.trim_start_matches('.').to_ascii_lowercase();
        match normalized.as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "txt" | "text" => Some(Self::PlainText),
            "pdf" => Some(Self::Pdf),
            "docx" => Some(Self::Docx),
            "doc" => Some(Self::Doc),
            "rtf" => Some(Self::Rtf),
            "odt" => Some(Self::Odt),
            _ => None,
        }
    }

    /// Identifies format from a file path based on its extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    /// The canonical primary file extension (without leading dot).
    pub const fn primary_extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::PlainText => "txt",
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Doc => "doc",
            Self::Rtf => "rtf",
            Self::Odt => "odt",
        }
    }

    /// Standard MIME media type for this format.
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown",
            Self::PlainText => "text/plain",
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Doc => "application/msword",
            Self::Rtf => "application/rtf",
            Self::Odt => "application/vnd.oasis.opendocument.text",
        }
    }

    /// Whether this format supports in-place atomic line edits.
    pub const fn is_direct_editable(self) -> bool {
        matches!(self, Self::Markdown | Self::PlainText)
    }

    /// Whether this format can be generated as a new export.
    pub const fn supports_export(self) -> bool {
        matches!(
            self,
            Self::Markdown | Self::PlainText | Self::Docx | Self::Rtf | Self::Odt
        )
    }

    /// Whether this format is legacy input-only.
    pub const fn is_legacy_input_only(self) -> bool {
        matches!(self, Self::Doc)
    }

    /// Inspects magic byte signatures at the start of a buffer.
    pub fn sniff_magic_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"%PDF-") {
            return Some(Self::Pdf);
        }
        if bytes.starts_with(b"{\\rtf") {
            return Some(Self::Rtf);
        }
        if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
            return Some(Self::Doc);
        }
        if bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
            // ZIP archive (DOCX or ODT container)
            // Check for ODT mimetype entry or DOCX [Content_Types].xml
            if bytes.windows(8).any(|window| window == b"mimetype")
                || bytes
                    .windows(28)
                    .any(|window| window == b"application/vnd.oasis.opendocument")
            {
                return Some(Self::Odt);
            }
            return Some(Self::Docx);
        }
        None
    }

    /// Validates that declared format is consistent with file bytes.
    pub fn validate_bytes(self, bytes: &[u8]) -> Result<(), DocumentFormatError> {
        if bytes.is_empty() {
            return Err(DocumentFormatError::EmptyFile);
        }
        match self {
            Self::Pdf => {
                if !bytes.starts_with(b"%PDF-") {
                    return Err(DocumentFormatError::MismatchedSignature {
                        expected: self,
                        found: "missing %PDF- header",
                    });
                }
                // Basic encrypted PDF detection
                if bytes.windows(8).any(|w| w == b"/Encrypt") {
                    return Err(DocumentFormatError::EncryptedFile);
                }
            }
            Self::Rtf => {
                if !bytes.starts_with(b"{\\rtf") {
                    return Err(DocumentFormatError::MismatchedSignature {
                        expected: self,
                        found: "missing {\\rtf header",
                    });
                }
            }
            Self::Doc => {
                if !bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
                    return Err(DocumentFormatError::MismatchedSignature {
                        expected: self,
                        found: "missing OLE compound document header",
                    });
                }
            }
            Self::Docx | Self::Odt => {
                if !bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
                    return Err(DocumentFormatError::MismatchedSignature {
                        expected: self,
                        found: "missing ZIP container signature",
                    });
                }
            }
            Self::Markdown | Self::PlainText => {
                // Must be valid UTF-8 and contain no null bytes
                let text =
                    std::str::from_utf8(bytes).map_err(|_| DocumentFormatError::InvalidEncoding)?;
                if text.contains('\0') {
                    return Err(DocumentFormatError::BinaryDataInText);
                }
            }
        }
        Ok(())
    }
}

impl fmt::Display for DocumentFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Markdown => write!(f, "Markdown"),
            Self::PlainText => write!(f, "Plain Text"),
            Self::Pdf => write!(f, "PDF"),
            Self::Docx => write!(f, "Word (.docx)"),
            Self::Doc => write!(f, "Word Legacy (.doc)"),
            Self::Rtf => write!(f, "Rich Text (.rtf)"),
            Self::Odt => write!(f, "OpenDocument (.odt)"),
        }
    }
}

/// Errors occurring during document format inspection or parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentFormatError {
    EmptyFile,
    InvalidEncoding,
    BinaryDataInText,
    EncryptedFile,
    MismatchedSignature {
        expected: DocumentFormat,
        found: &'static str,
    },
    UnsupportedFormat(String),
}

impl fmt::Display for DocumentFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFile => write!(f, "document is empty (0 bytes)"),
            Self::InvalidEncoding => write!(f, "document is not valid UTF-8 text"),
            Self::BinaryDataInText => write!(f, "text document contains invalid null/binary bytes"),
            Self::EncryptedFile => write!(
                f,
                "document is password-protected or encrypted; encrypted files are rejected"
            ),
            Self::MismatchedSignature { expected, found } => {
                write!(
                    f,
                    "declared {expected} format does not match file signature: {found}"
                )
            }
            Self::UnsupportedFormat(ext) => write!(f, "unsupported file format extension: .{ext}"),
        }
    }
}

impl std::error::Error for DocumentFormatError {}

/// Canonical document representation parsed from any supported word-processing format.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Document {
    pub metadata: DocumentMetadata,
    pub source: DocumentSource,
    pub blocks: Vec<Block>,
}

impl Document {
    pub fn new(source: DocumentSource, metadata: DocumentMetadata, blocks: Vec<Block>) -> Self {
        Self {
            metadata,
            source,
            blocks,
        }
    }

    /// Returns the plain text concatenation of all semantic blocks in the document.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for (i, block) in self.blocks.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            out.push_str(&block.plain_text());
        }
        out
    }

    /// Approximate total word count across all document blocks.
    pub fn total_word_count(&self) -> usize {
        self.blocks.iter().map(|b| b.word_count()).sum()
    }

    /// Approximate token count using conservative word/char heuristics (approx 1 token per 4 chars).
    pub fn approx_token_count(&self) -> usize {
        let chars: usize = self.blocks.iter().map(|b| b.char_count()).sum();
        chars.div_ceil(4).max(1)
    }

    /// Extracts all structural headings with their level, title text, and source anchor.
    pub fn headings(&self) -> Vec<(u8, String, SourceAnchor)> {
        let mut headings = Vec::new();
        for block in &self.blocks {
            if let Block::Heading {
                level,
                runs,
                anchor,
            } = block
            {
                let title = runs.iter().map(|r| r.text()).collect::<Vec<_>>().join("");
                headings.push((*level, title, anchor.clone()));
            }
        }
        headings
    }
}

/// Metadata extracted from document properties or file headers.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// Provenance and integrity of the underlying source document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentSource {
    pub display_name: String,
    pub content_digest: String,
    pub format: DocumentFormat,
    pub byte_size: u64,
}

impl DocumentSource {
    pub fn from_bytes(display_name: &str, format: DocumentFormat, bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = format!("{:x}", hasher.finalize());
        Self {
            display_name: display_name.to_string(),
            content_digest: digest,
            format,
            byte_size: bytes.len() as u64,
        }
    }
}

/// Precise anchor linking an AST block to its source position.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceAnchor {
    pub page: Option<u32>,
    pub section: Option<String>,
    pub block_index: usize,
    pub line_range: Option<(usize, usize)>,
}

/// Structural block element within a canonical document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Block {
    Heading {
        level: u8,
        runs: Vec<InlineRun>,
        anchor: SourceAnchor,
    },
    Paragraph {
        runs: Vec<InlineRun>,
        anchor: SourceAnchor,
    },
    List {
        ordered: bool,
        items: Vec<ListItem>,
        anchor: SourceAnchor,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        anchor: SourceAnchor,
    },
    Code {
        language: Option<String>,
        text: String,
        anchor: SourceAnchor,
    },
    PageBreak {
        page: u32,
    },
    Unsupported {
        description: String,
        anchor: SourceAnchor,
    },
}

impl Block {
    pub fn anchor(&self) -> Option<&SourceAnchor> {
        match self {
            Self::Heading { anchor, .. }
            | Self::Paragraph { anchor, .. }
            | Self::List { anchor, .. }
            | Self::Table { anchor, .. }
            | Self::Code { anchor, .. }
            | Self::Unsupported { anchor, .. } => Some(anchor),
            Self::PageBreak { .. } => None,
        }
    }

    pub fn plain_text(&self) -> String {
        match self {
            Self::Heading { runs, .. } | Self::Paragraph { runs, .. } => {
                runs.iter().map(|r| r.text()).collect::<Vec<_>>().join("")
            }
            Self::List { ordered, items, .. } => {
                let mut out = String::new();
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        out.push('\n');
                    }
                    if *ordered {
                        out.push_str(&format!("{}. ", idx + 1));
                    } else {
                        out.push_str("- ");
                    }
                    out.push_str(&item.plain_text());
                }
                out
            }
            Self::Table { headers, rows, .. } => {
                let mut out = String::new();
                if !headers.is_empty() {
                    out.push_str(&headers.join(" | "));
                    out.push('\n');
                }
                for row in rows {
                    out.push_str(&row.join(" | "));
                    out.push('\n');
                }
                out.trim_end().to_string()
            }
            Self::Code { text, .. } => text.clone(),
            Self::PageBreak { page } => format!("[Page {page}]"),
            Self::Unsupported { description, .. } => format!("[Unsupported: {description}]"),
        }
    }

    pub fn word_count(&self) -> usize {
        self.plain_text()
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .count()
    }

    pub fn char_count(&self) -> usize {
        self.plain_text().chars().count()
    }
}

/// An individual item in an ordered or unordered list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ListItem {
    pub runs: Vec<InlineRun>,
    pub sub_items: Vec<ListItem>,
}

impl ListItem {
    pub fn new(runs: Vec<InlineRun>) -> Self {
        Self {
            runs,
            sub_items: Vec::new(),
        }
    }

    pub fn plain_text(&self) -> String {
        let mut out = self
            .runs
            .iter()
            .map(|r| r.text())
            .collect::<Vec<_>>()
            .join("");
        for sub in &self.sub_items {
            out.push_str("\n  - ");
            out.push_str(&sub.plain_text());
        }
        out
    }
}

/// Inline text run with optional semantic styling.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InlineRun {
    Text(String),
    Emphasis(String),
    Strong(String),
    Link { text: String, url: String },
}

impl InlineRun {
    pub fn text(&self) -> &str {
        match self {
            Self::Text(s) | Self::Emphasis(s) | Self::Strong(s) => s.as_str(),
            Self::Link { text, .. } => text.as_str(),
        }
    }
}

/// Native in-process parser for plain text documents (`.txt`).
pub fn parse_plain_text(display_name: &str, bytes: &[u8]) -> Result<Document, DocumentFormatError> {
    DocumentFormat::PlainText.validate_bytes(bytes)?;
    let text = std::str::from_utf8(bytes).map_err(|_| DocumentFormatError::InvalidEncoding)?;
    let source = DocumentSource::from_bytes(display_name, DocumentFormat::PlainText, bytes);
    let mut blocks = Vec::new();
    let mut block_index = 0;
    let mut current_line = 1;

    for paragraph in text.split("\n\n") {
        let trimmed = paragraph.trim();
        if !trimmed.is_empty() {
            let line_count = paragraph.lines().count().max(1);
            let end_line = current_line + line_count - 1;
            blocks.push(Block::Paragraph {
                runs: vec![InlineRun::Text(trimmed.to_string())],
                anchor: SourceAnchor {
                    page: None,
                    section: None,
                    block_index,
                    line_range: Some((current_line, end_line)),
                },
            });
            block_index += 1;
            current_line = end_line + 2; // account for split \n\n
        } else {
            current_line += 2;
        }
    }

    Ok(Document::new(source, DocumentMetadata::default(), blocks))
}

/// Native in-process parser for Markdown documents (`.md`).
pub fn parse_markdown(display_name: &str, bytes: &[u8]) -> Result<Document, DocumentFormatError> {
    DocumentFormat::Markdown.validate_bytes(bytes)?;
    let text = std::str::from_utf8(bytes).map_err(|_| DocumentFormatError::InvalidEncoding)?;
    let source = DocumentSource::from_bytes(display_name, DocumentFormat::Markdown, bytes);
    let mut blocks = Vec::new();
    let mut block_index = 0;
    let mut current_section: Option<String> = None;

    let lines: Vec<&str> = text.lines().collect();
    let mut idx = 0;

    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        // Code fences
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let fence_marker = &trimmed[..3];
            let language = trimmed[3..].trim();
            let lang_opt = if language.is_empty() {
                None
            } else {
                Some(language.to_string())
            };
            let start_line = idx + 1;
            idx += 1;
            let mut code_lines = Vec::new();
            while idx < lines.len() {
                let cur = lines[idx];
                if cur.trim().starts_with(fence_marker) {
                    idx += 1;
                    break;
                }
                code_lines.push(cur);
                idx += 1;
            }
            let end_line = idx;
            blocks.push(Block::Code {
                language: lang_opt,
                text: code_lines.join("\n"),
                anchor: SourceAnchor {
                    page: None,
                    section: current_section.clone(),
                    block_index,
                    line_range: Some((start_line, end_line)),
                },
            });
            block_index += 1;
            continue;
        }

        // Headings (# Heading)
        if trimmed.starts_with('#') {
            let hash_count = trimmed.chars().take_while(|c| *c == '#').count();
            if hash_count <= 6 && trimmed[hash_count..].starts_with(' ') {
                let heading_text = trimmed[hash_count..].trim();
                current_section = Some(heading_text.to_string());
                let line_num = idx + 1;
                blocks.push(Block::Heading {
                    level: hash_count as u8,
                    runs: parse_inline_markdown(heading_text),
                    anchor: SourceAnchor {
                        page: None,
                        section: current_section.clone(),
                        block_index,
                        line_range: Some((line_num, line_num)),
                    },
                });
                block_index += 1;
                idx += 1;
                continue;
            }
        }

        // Tables (| col | col |)
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let start_line = idx + 1;
            let mut table_rows = Vec::new();
            while idx < lines.len()
                && lines[idx].trim().starts_with('|')
                && lines[idx].trim().ends_with('|')
            {
                let row_str = lines[idx].trim();
                // Check if separator line (| --- | --- |)
                let is_sep = row_str.trim_matches('|').split('|').all(|cell| {
                    cell.trim()
                        .chars()
                        .all(|c| c == '-' || c == ':' || c.is_whitespace())
                });
                if !is_sep {
                    let cells: Vec<String> = row_str
                        .trim_matches('|')
                        .split('|')
                        .map(|cell| cell.trim().to_string())
                        .collect();
                    table_rows.push(cells);
                }
                idx += 1;
            }
            let end_line = idx;
            let (headers, rows) = if !table_rows.is_empty() {
                let headers = table_rows.remove(0);
                (headers, table_rows)
            } else {
                (Vec::new(), Vec::new())
            };
            blocks.push(Block::Table {
                headers,
                rows,
                anchor: SourceAnchor {
                    page: None,
                    section: current_section.clone(),
                    block_index,
                    line_range: Some((start_line, end_line)),
                },
            });
            block_index += 1;
            continue;
        }

        // Lists (- item or 1. item)
        if is_list_item(trimmed) {
            let start_line = idx + 1;
            let is_ordered = trimmed.chars().next().is_some_and(|c| c.is_ascii_digit());
            let mut items = Vec::new();
            while idx < lines.len() && is_list_item(lines[idx].trim()) {
                let cur = lines[idx].trim();
                let text = strip_list_marker(cur);
                items.push(ListItem::new(parse_inline_markdown(text)));
                idx += 1;
            }
            let end_line = idx;
            blocks.push(Block::List {
                ordered: is_ordered,
                items,
                anchor: SourceAnchor {
                    page: None,
                    section: current_section.clone(),
                    block_index,
                    line_range: Some((start_line, end_line)),
                },
            });
            block_index += 1;
            continue;
        }

        // Paragraph
        let start_line = idx + 1;
        let mut p_lines = Vec::new();
        while idx < lines.len() {
            let cur = lines[idx].trim();
            if cur.is_empty()
                || cur.starts_with('#')
                || cur.starts_with("```")
                || (cur.starts_with('|') && cur.ends_with('|'))
                || is_list_item(cur)
            {
                break;
            }
            p_lines.push(cur);
            idx += 1;
        }
        let end_line = idx;
        let p_text = p_lines.join(" ");
        blocks.push(Block::Paragraph {
            runs: parse_inline_markdown(&p_text),
            anchor: SourceAnchor {
                page: None,
                section: current_section.clone(),
                block_index,
                line_range: Some((start_line, end_line)),
            },
        });
        block_index += 1;
    }

    Ok(Document::new(source, DocumentMetadata::default(), blocks))
}

fn is_list_item(s: &str) -> bool {
    s.starts_with("- ")
        || s.starts_with("* ")
        || (s.len() >= 3
            && s.chars().next().is_some_and(|c| c.is_ascii_digit())
            && s.contains(". "))
}

fn strip_list_marker(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
        return rest.trim();
    }
    if let Some(pos) = s.find(". ") {
        let prefix = &s[..pos];
        if prefix.chars().all(|c| c.is_ascii_digit()) {
            return s[pos + 2..].trim();
        }
    }
    s.trim()
}

/// Parses inline formatting into structured InlineRun items:
/// `**strong**`, `*emphasis*`, `[link](url)`.
fn parse_inline_markdown(text: &str) -> Vec<InlineRun> {
    let mut runs = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Link: [text](url)
        if let Some(link_start) = remaining.find('[') {
            if let Some(link_mid) = remaining[link_start..].find("](") {
                let link_mid_abs = link_start + link_mid;
                if let Some(link_end) = remaining[link_mid_abs..].find(')') {
                    let link_end_abs = link_mid_abs + link_end;
                    // Flush preceding text before link
                    if link_start > 0 {
                        runs.extend(parse_styling(&remaining[..link_start]));
                    }
                    let link_text = &remaining[link_start + 1..link_mid_abs];
                    let link_url = &remaining[link_mid_abs + 2..link_end_abs];
                    runs.push(InlineRun::Link {
                        text: link_text.to_string(),
                        url: link_url.to_string(),
                    });
                    remaining = &remaining[link_end_abs + 1..];
                    continue;
                }
            }
        }
        // No more links; parse styling for remainder
        runs.extend(parse_styling(remaining));
        break;
    }

    if runs.is_empty() {
        runs.push(InlineRun::Text(text.to_string()));
    }
    runs
}

fn parse_styling(text: &str) -> Vec<InlineRun> {
    let mut runs = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Strong: **text**
        if let Some(start) = remaining.find("**") {
            if let Some(end) = remaining[start + 2..].find("**") {
                let end_abs = start + 2 + end;
                if start > 0 {
                    runs.extend(parse_emphasis(&remaining[..start]));
                }
                let strong_text = &remaining[start + 2..end_abs];
                runs.push(InlineRun::Strong(strong_text.to_string()));
                remaining = &remaining[end_abs + 2..];
                continue;
            }
        }
        runs.extend(parse_emphasis(remaining));
        break;
    }
    runs
}

fn parse_emphasis(text: &str) -> Vec<InlineRun> {
    let mut runs = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        // Emphasis: *text* (must not be preceded or followed by another *)
        if let Some(start) = remaining.find('*') {
            if let Some(end) = remaining[start + 1..].find('*') {
                let end_abs = start + 1 + end;
                if start > 0 {
                    runs.push(InlineRun::Text(remaining[..start].to_string()));
                }
                let em_text = &remaining[start + 1..end_abs];
                runs.push(InlineRun::Emphasis(em_text.to_string()));
                remaining = &remaining[end_abs + 1..];
                continue;
            }
        }
        if !remaining.is_empty() {
            runs.push(InlineRun::Text(remaining.to_string()));
        }
        break;
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_detection_from_extensions_and_paths() {
        assert_eq!(
            DocumentFormat::from_extension("md"),
            Some(DocumentFormat::Markdown)
        );
        assert_eq!(
            DocumentFormat::from_extension(".txt"),
            Some(DocumentFormat::PlainText)
        );
        assert_eq!(
            DocumentFormat::from_extension("PDF"),
            Some(DocumentFormat::Pdf)
        );
        assert_eq!(
            DocumentFormat::from_extension("docx"),
            Some(DocumentFormat::Docx)
        );
        assert_eq!(
            DocumentFormat::from_extension("doc"),
            Some(DocumentFormat::Doc)
        );
        assert_eq!(
            DocumentFormat::from_extension("rtf"),
            Some(DocumentFormat::Rtf)
        );
        assert_eq!(
            DocumentFormat::from_extension("odt"),
            Some(DocumentFormat::Odt)
        );
        assert_eq!(DocumentFormat::from_extension("exe"), None);
        assert_eq!(DocumentFormat::from_extension("png"), None);

        assert_eq!(
            DocumentFormat::from_path(Path::new("report.DOCX")),
            Some(DocumentFormat::Docx)
        );
        assert_eq!(
            DocumentFormat::from_path(Path::new("/home/user/notes.md")),
            Some(DocumentFormat::Markdown)
        );
    }

    #[test]
    fn format_capabilities_match_v1_specification() {
        // Markdown and PlainText are direct editable
        assert!(DocumentFormat::Markdown.is_direct_editable());
        assert!(DocumentFormat::PlainText.is_direct_editable());
        assert!(!DocumentFormat::Pdf.is_direct_editable());
        assert!(!DocumentFormat::Docx.is_direct_editable());
        assert!(!DocumentFormat::Doc.is_direct_editable());

        // Exportable formats
        assert!(DocumentFormat::Markdown.supports_export());
        assert!(DocumentFormat::PlainText.supports_export());
        assert!(DocumentFormat::Docx.supports_export());
        assert!(DocumentFormat::Rtf.supports_export());
        assert!(DocumentFormat::Odt.supports_export());
        assert!(!DocumentFormat::Pdf.supports_export());
        assert!(!DocumentFormat::Doc.supports_export());

        // Legacy input only
        assert!(DocumentFormat::Doc.is_legacy_input_only());
        assert!(!DocumentFormat::Docx.is_legacy_input_only());
    }

    #[test]
    fn magic_bytes_sniffing_identifies_binary_formats() {
        assert_eq!(
            DocumentFormat::sniff_magic_bytes(b"%PDF-1.7\n..."),
            Some(DocumentFormat::Pdf)
        );
        assert_eq!(
            DocumentFormat::sniff_magic_bytes(b"{\\rtf1\\ansi..."),
            Some(DocumentFormat::Rtf)
        );
        assert_eq!(
            DocumentFormat::sniff_magic_bytes(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]),
            Some(DocumentFormat::Doc)
        );
        assert_eq!(
            DocumentFormat::sniff_magic_bytes(&[0x50, 0x4B, 0x03, 0x04, 0x00, 0x00]),
            Some(DocumentFormat::Docx)
        );
        assert_eq!(DocumentFormat::sniff_magic_bytes(b"Hello world"), None);
    }

    #[test]
    fn byte_validation_enforces_signatures_and_rejects_encrypted() {
        assert!(DocumentFormat::Pdf.validate_bytes(b"%PDF-1.4 fine").is_ok());
        assert_eq!(
            DocumentFormat::Pdf.validate_bytes(b"not-a-pdf"),
            Err(DocumentFormatError::MismatchedSignature {
                expected: DocumentFormat::Pdf,
                found: "missing %PDF- header"
            })
        );
        assert_eq!(
            DocumentFormat::Pdf.validate_bytes(b"%PDF-1.4\n1 0 obj <</Encrypt 2 0 R>>"),
            Err(DocumentFormatError::EncryptedFile)
        );
        assert_eq!(
            DocumentFormat::PlainText.validate_bytes(b"text with \0 null byte"),
            Err(DocumentFormatError::BinaryDataInText)
        );
        assert!(
            DocumentFormat::PlainText
                .validate_bytes(b"Clean UTF-8 text")
                .is_ok()
        );
    }

    #[test]
    fn plain_text_parser_produces_canonical_paragraphs() {
        let content = b"First paragraph with some words.\n\nSecond paragraph has more text.";
        let doc = parse_plain_text("notes.txt", content).unwrap();

        assert_eq!(doc.source.display_name, "notes.txt");
        assert_eq!(doc.source.format, DocumentFormat::PlainText);
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(
            doc.plain_text(),
            "First paragraph with some words.\n\nSecond paragraph has more text."
        );
        assert_eq!(doc.total_word_count(), 10);
        assert!(doc.approx_token_count() > 0);

        if let Block::Paragraph { anchor, .. } = &doc.blocks[0] {
            assert_eq!(anchor.block_index, 0);
            assert_eq!(anchor.line_range, Some((1, 1)));
        } else {
            panic!("expected paragraph block");
        }
    }

    #[test]
    fn markdown_parser_extracts_headings_lists_tables_and_code() {
        let md = r#"# Document Title

This is a paragraph with **strong** text, *emphasis*, and a [Dociler link](https://github.com/deanyouknow/llm-docs).

## Section 1: Code and Lists

```rust
fn main() {
    println!("hello");
}
```

- First item
- Second item

| Name | Role |
| --- | --- |
| Alice | Admin |
| Bob | User |
"#;

        let doc = parse_markdown("sample.md", md.as_bytes()).unwrap();
        assert_eq!(doc.source.display_name, "sample.md");
        assert_eq!(doc.source.format, DocumentFormat::Markdown);

        let headings = doc.headings();
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].0, 1);
        assert_eq!(headings[0].1, "Document Title");
        assert_eq!(headings[1].0, 2);
        assert_eq!(headings[1].1, "Section 1: Code and Lists");

        // Verify block count: H1, Paragraph, H2, Code, List, Table = 6 blocks
        assert_eq!(doc.blocks.len(), 6);

        // Verify code block
        if let Block::Code {
            language,
            text,
            anchor,
        } = &doc.blocks[3]
        {
            assert_eq!(language.as_deref(), Some("rust"));
            assert!(text.contains("println!"));
            assert_eq!(anchor.section.as_deref(), Some("Section 1: Code and Lists"));
        } else {
            panic!("expected Code block at index 3");
        }

        // Verify table
        if let Block::Table { headers, rows, .. } = &doc.blocks[5] {
            assert_eq!(headers, &["Name", "Role"]);
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0], &["Alice", "Admin"]);
            assert_eq!(rows[1], &["Bob", "User"]);
        } else {
            panic!("expected Table block at index 5");
        }
    }
}
