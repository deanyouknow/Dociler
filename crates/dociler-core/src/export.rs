//! Canonical document AST export pipeline.
//!
//! Renders canonical `Document` ASTs into target export formats:
//! - Markdown (`.md`)
//! - PlainText (`.txt`)
//! - Microsoft Word (`.docx`)
//! - Rich Text Format (`.rtf`)
//! - OpenDocument Text (`.odt`)
//!
//! Enforces:
//! - Refusal to overwrite existing binary source documents (PDF, DOC, DOCX, RTF, ODT).
//! - Workspace filesystem boundary containment.
//! - Atomic publication using same-directory temporary files.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::document::{Block, Document, DocumentFormat, InlineRun};
use crate::workspace::{DiscoveryError, Workspace};

/// Target format for document export from canonical AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    PlainText,
    Docx,
    Rtf,
    Odt,
}

impl ExportFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::PlainText => "txt",
            Self::Docx => "docx",
            Self::Rtf => "rtf",
            Self::Odt => "odt",
        }
    }

    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown",
            Self::PlainText => "text/plain",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Rtf => "application/rtf",
            Self::Odt => "application/vnd.oasis.opendocument.text",
        }
    }
}

/// Errors occurring during document export.
#[derive(Debug)]
pub enum ExportError {
    /// Overwriting existing binary source files is prohibited.
    BinarySourceOverwriteDenied { path: PathBuf },
    /// Destination path resolves outside the workspace boundary.
    PathEscape { path: PathBuf },
    /// Standard I/O failure.
    Io(io::Error),
    /// Zip container creation failure.
    Zip(zip::result::ZipError),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BinarySourceOverwriteDenied { path } => write!(
                f,
                "refusing to overwrite existing binary source document '{}'",
                path.display()
            ),
            Self::PathEscape { path } => write!(
                f,
                "export destination '{}' escapes workspace boundary",
                path.display()
            ),
            Self::Io(err) => write!(f, "export I/O error: {err}"),
            Self::Zip(err) => write!(f, "export zip packaging error: {err}"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Zip(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for ExportError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<zip::result::ZipError> for ExportError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

impl From<DiscoveryError> for ExportError {
    fn from(err: DiscoveryError) -> Self {
        match err {
            DiscoveryError::PathEscape { symlink, .. } => Self::PathEscape { path: symlink },
            DiscoveryError::Io(err) => Self::Io(err),
            _ => Self::Io(io::Error::new(io::ErrorKind::Other, err.to_string())),
        }
    }
}

/// Summary report of a completed document export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub destination_path: PathBuf,
    pub format: ExportFormat,
    pub byte_size: usize,
    pub warnings: Vec<String>,
}

/// Exports a canonical `Document` to the specified format and destination path.
pub fn export_document(
    workspace: &Workspace,
    document: &Document,
    format: ExportFormat,
    relative_path: &Path,
    allow_overwrite: bool,
) -> Result<ExportReport, ExportError> {
    // 1. Verify workspace containment
    let target_path = if relative_path.is_relative() {
        workspace.root().join(relative_path)
    } else {
        relative_path.to_path_buf()
    };

    if target_path.exists() {
        let canonical = target_path.canonicalize().map_err(ExportError::Io)?;
        if !canonical.starts_with(workspace.root()) {
            return Err(ExportError::PathEscape {
                path: relative_path.to_path_buf(),
            });
        }

        // Refuse to overwrite existing binary source documents
        let existing_fmt =
            DocumentFormat::from_path(&target_path).unwrap_or(DocumentFormat::PlainText);
        if !existing_fmt.is_direct_editable() && !allow_overwrite {
            return Err(ExportError::BinarySourceOverwriteDenied {
                path: relative_path.to_path_buf(),
            });
        }
    } else {
        let parent = target_path.parent().unwrap_or_else(|| workspace.root());
        fs::create_dir_all(parent)?;
        let canonical_parent = parent.canonicalize().map_err(ExportError::Io)?;
        if !canonical_parent.starts_with(workspace.root()) {
            return Err(ExportError::PathEscape {
                path: relative_path.to_path_buf(),
            });
        }
    }

    // 2. Render document bytes according to format
    let mut warnings = Vec::new();
    for block in &document.blocks {
        if let Block::Unsupported { description, .. } = block {
            warnings.push(format!("Flattened unsupported element: {description}"));
        }
    }

    let bytes = match format {
        ExportFormat::Markdown => render_markdown(document).into_bytes(),
        ExportFormat::PlainText => render_plain_text(document).into_bytes(),
        ExportFormat::Docx => render_docx(document)?,
        ExportFormat::Rtf => render_rtf(document),
        ExportFormat::Odt => render_odt(document)?,
    };

    let byte_size = bytes.len();

    // 3. Atomically write via same-directory temp file and rename
    atomic_write_file(&target_path, &bytes)?;

    Ok(ExportReport {
        destination_path: relative_path.to_path_buf(),
        format,
        byte_size,
        warnings,
    })
}

/// Renders a canonical `Document` to Markdown string.
pub fn render_markdown(document: &Document) -> String {
    let mut out = String::new();

    for (i, block) in document.blocks.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        match block {
            Block::Heading { level, runs, .. } => {
                let prefix = "#".repeat(*level as usize);
                out.push_str(&prefix);
                out.push(' ');
                for run in runs {
                    out.push_str(&render_inline_markdown(run));
                }
            }
            Block::Paragraph { runs, .. } => {
                for run in runs {
                    out.push_str(&render_inline_markdown(run));
                }
            }
            Block::List { ordered, items, .. } => {
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        out.push('\n');
                    }
                    if *ordered {
                        out.push_str(&format!("{}. ", idx + 1));
                    } else {
                        out.push_str("- ");
                    }
                    for run in &item.runs {
                        out.push_str(&render_inline_markdown(run));
                    }
                }
            }
            Block::Table { headers, rows, .. } => {
                out.push_str("| ");
                out.push_str(&headers.join(" | "));
                out.push_str(" |\n|");
                for _ in headers {
                    out.push_str("---|");
                }
                out.push('\n');
                for (r_idx, row) in rows.iter().enumerate() {
                    if r_idx > 0 {
                        out.push('\n');
                    }
                    out.push_str("| ");
                    out.push_str(&row.join(" | "));
                    out.push_str(" |");
                }
            }
            Block::Code { language, text, .. } => {
                out.push_str("```");
                if let Some(lang) = language {
                    out.push_str(lang);
                }
                out.push('\n');
                out.push_str(text);
                out.push_str("\n```");
            }
            Block::PageBreak { .. } => {
                out.push_str("---");
            }
            Block::Unsupported { description, .. } => {
                out.push_str(&format!("<!-- Unsupported element: {description} -->"));
            }
        }
    }

    out
}

fn render_inline_markdown(run: &InlineRun) -> String {
    match run {
        InlineRun::Text(s) => s.clone(),
        InlineRun::Strong(s) => format!("**{s}**"),
        InlineRun::Emphasis(s) => format!("*{s}*"),
        InlineRun::Link { text, url } => format!("[{text}]({url})"),
    }
}

/// Renders a canonical `Document` to plain text string.
pub fn render_plain_text(document: &Document) -> String {
    document.plain_text()
}

/// Renders a canonical `Document` to valid RTF 1.5 document bytes.
pub fn render_rtf(document: &Document) -> Vec<u8> {
    let mut out = String::new();
    out.push_str(r"{\rtf1\ansi\ansicpg1252\deff0\nouicompat");
    out.push('\n');
    out.push_str(r"{\fonttbl{\f0\fnil\fcharset0 Arial;}{\f1\fnil\fcharset0 Courier New;}}");
    out.push('\n');
    out.push_str(r"{\colortbl ;\red0\green0\blue0;\red0\green0\blue255;}");
    out.push('\n');
    out.push_str(r"\viewkind4\uc1\pard\sa200\sl276\slmult1\f0\fs22\lang9");
    out.push('\n');

    for block in &document.blocks {
        match block {
            Block::Heading { level, runs, .. } => {
                let fs = match level {
                    1 => 36,
                    2 => 30,
                    _ => 26,
                };
                out.push_str(&format!(r"\pard\sa200\b\fs{fs} "));
                for run in runs {
                    out.push_str(&rtf_escape(run.text()));
                }
                out.push_str(r"\b0\fs22\par");
                out.push('\n');
            }
            Block::Paragraph { runs, .. } => {
                out.push_str(r"\pard\sa200 ");
                for run in runs {
                    match run {
                        InlineRun::Text(s) => out.push_str(&rtf_escape(s)),
                        InlineRun::Strong(s) => {
                            out.push_str(r"\b ");
                            out.push_str(&rtf_escape(s));
                            out.push_str(r"\b0 ");
                        }
                        InlineRun::Emphasis(s) => {
                            out.push_str(r"\i ");
                            out.push_str(&rtf_escape(s));
                            out.push_str(r"\i0 ");
                        }
                        InlineRun::Link { text, .. } => {
                            out.push_str(r"\cf2\ul ");
                            out.push_str(&rtf_escape(text));
                            out.push_str(r"\ul0\cf1 ");
                        }
                    }
                }
                out.push_str(r"\par");
                out.push('\n');
            }
            Block::List { items, .. } => {
                for item in items {
                    out.push_str(r"\pard\sa100\fi-360\li720\bullet\tab ");
                    for run in &item.runs {
                        out.push_str(&rtf_escape(run.text()));
                    }
                    out.push_str(r"\par");
                    out.push('\n');
                }
            }
            Block::Table { headers, rows, .. } => {
                let col_width = 2500;
                // Header row
                out.push_str(r"\trowd\trgaph108\trleft-108");
                for (idx, _) in headers.iter().enumerate() {
                    out.push_str(&format!(r"\cellx{}", (idx + 1) * col_width));
                }
                for header in headers {
                    out.push_str(r"\intbl\b ");
                    out.push_str(&rtf_escape(header));
                    out.push_str(r"\b0\cell ");
                }
                out.push_str(r"\row");
                out.push('\n');

                // Data rows
                for row in rows {
                    out.push_str(r"\trowd\trgaph108\trleft-108");
                    for (idx, _) in row.iter().enumerate() {
                        out.push_str(&format!(r"\cellx{}", (idx + 1) * col_width));
                    }
                    for cell in row {
                        out.push_str(r"\intbl ");
                        out.push_str(&rtf_escape(cell));
                        out.push_str(r"\cell ");
                    }
                    out.push_str(r"\row");
                    out.push('\n');
                }
            }
            Block::Code { text, .. } => {
                out.push_str(r"\pard\sa200\f1\fs18 ");
                for line in text.lines() {
                    out.push_str(&rtf_escape(line));
                    out.push_str(r"\line ");
                }
                out.push_str(r"\f0\fs22\par");
                out.push('\n');
            }
            Block::PageBreak { .. } => {
                out.push_str(r"\page ");
            }
            Block::Unsupported { description, .. } => {
                out.push_str(&format!(r"\pard\sa200 [{}]\par\n", rtf_escape(description)));
            }
        }
    }

    out.push('}');
    out.into_bytes()
}

fn rtf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '{' => out.push_str(r"\{"),
            '}' => out.push_str(r"\}"),
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\line "),
            c if c.is_ascii() => out.push(c),
            c => out.push_str(&format!(r"\u{}?", c as u32)),
        }
    }
    out
}

/// Renders a canonical `Document` to a valid Microsoft Word `.docx` ZIP archive.
pub fn render_docx(document: &Document) -> Result<Vec<u8>, ExportError> {
    let mut cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // 1. [Content_Types].xml
    zip.start_file("[Content_Types].xml", options)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#,
    )?;

    // 2. _rels/.rels
    zip.start_file("_rels/.rels", options)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"#,
    )?;

    // 3. docProps/core.xml
    zip.start_file("docProps/core.xml", options)?;
    let title = xml_escape(
        document
            .metadata
            .title
            .as_deref()
            .unwrap_or("Dociler Document"),
    );
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/">
  <dc:title>{title}</dc:title>
  <dc:creator>Dociler</dc:creator>
</cp:coreProperties>"#
        )
        .as_bytes(),
    )?;

    // 4. word/document.xml
    zip.start_file("word/document.xml", options)?;
    let mut doc_xml = String::new();
    doc_xml.push_str(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
"#,
    );

    for block in &document.blocks {
        match block {
            Block::Heading { level, runs, .. } => {
                doc_xml.push_str(&format!(
                    r#"    <w:p><w:pPr><w:pStyle w:val="Heading{level}"/></w:pPr>"#
                ));
                for run in runs {
                    doc_xml.push_str(&format!(
                        r#"<w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r>"#,
                        xml_escape(run.text())
                    ));
                }
                doc_xml.push_str("</w:p>\n");
            }
            Block::Paragraph { runs, .. } => {
                doc_xml.push_str("    <w:p>");
                for run in runs {
                    let mut r_pr = String::new();
                    match run {
                        InlineRun::Strong(_) => r_pr.push_str("<w:b/>"),
                        InlineRun::Emphasis(_) => r_pr.push_str("<w:i/>"),
                        _ => {}
                    }
                    doc_xml.push_str(&format!(
                        r#"<w:r><w:rPr>{r_pr}</w:rPr><w:t xml:space="preserve">{}</w:t></w:r>"#,
                        xml_escape(run.text())
                    ));
                }
                doc_xml.push_str("</w:p>\n");
            }
            Block::List { items, .. } => {
                for item in items {
                    doc_xml
                        .push_str(r#"    <w:p><w:pPr><w:pStyle w:val="ListParagraph"/></w:pPr>"#);
                    for run in &item.runs {
                        doc_xml.push_str(&format!(
                            r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
                            xml_escape(run.text())
                        ));
                    }
                    doc_xml.push_str("</w:p>\n");
                }
            }
            Block::Table { headers, rows, .. } => {
                doc_xml.push_str("    <w:tbl>\n");
                // Header row
                doc_xml.push_str("      <w:tr>\n");
                for h in headers {
                    doc_xml.push_str(&format!(
                        r#"        <w:tc><w:p><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:tc>
"#,
                        xml_escape(h)
                    ));
                }
                doc_xml.push_str("      </w:tr>\n");
                // Body rows
                for row in rows {
                    doc_xml.push_str("      <w:tr>\n");
                    for cell in row {
                        doc_xml.push_str(&format!(
                            r#"        <w:tc><w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:tc>
"#,
                            xml_escape(cell)
                        ));
                    }
                    doc_xml.push_str("      </w:tr>\n");
                }
                doc_xml.push_str("    </w:tbl>\n");
            }
            Block::Code { text, .. } => {
                doc_xml.push_str("    <w:p>");
                doc_xml.push_str(&format!(
                    r#"<w:r><w:rPr><w:rFonts w:ascii="Courier New"/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r>"#,
                    xml_escape(text)
                ));
                doc_xml.push_str("</w:p>\n");
            }
            Block::PageBreak { .. } => {
                doc_xml.push_str(r#"    <w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
                doc_xml.push('\n');
            }
            Block::Unsupported { description, .. } => {
                doc_xml.push_str(&format!(
                    r#"    <w:p><w:r><w:t xml:space="preserve">[{}]</w:t></w:r></w:p>
"#,
                    xml_escape(description)
                ));
            }
        }
    }

    doc_xml.push_str(
        r#"    <w:sectPr/>
  </w:body>
</w:document>"#,
    );

    zip.write_all(doc_xml.as_bytes())?;
    zip.finish()?;

    Ok(cursor.into_inner())
}

/// Renders a canonical `Document` to a valid OpenDocument Text `.odt` ZIP archive.
pub fn render_odt(document: &Document) -> Result<Vec<u8>, ExportError> {
    let mut cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut cursor);

    // 1. mimetype (stored uncompressed as first entry)
    let stored_options =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored_options)?;
    zip.write_all(b"application/vnd.oasis.opendocument.text")?;

    let deflated_options =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // 2. META-INF/manifest.xml
    zip.start_file("META-INF/manifest.xml", deflated_options)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">
  <manifest:file-entry manifest:full-path="/" manifest:version="1.2" manifest:media-type="application/vnd.oasis.opendocument.text"/>
  <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
  <manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>
</manifest:manifest>"#,
    )?;

    // 3. meta.xml
    zip.start_file("meta.xml", deflated_options)?;
    let title = xml_escape(
        document
            .metadata
            .title
            .as_deref()
            .unwrap_or("Dociler Document"),
    );
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" office:version="1.2">
  <office:meta>
    <dc:title>{title}</dc:title>
    <dc:creator>Dociler</dc:creator>
  </office:meta>
</office:document-meta>"#
        )
        .as_bytes(),
    )?;

    // 4. content.xml
    zip.start_file("content.xml", deflated_options)?;
    let mut content_xml = String::new();
    content_xml.push_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
  office:version="1.2">
  <office:body>
    <office:text>
"#,
    );

    for block in &document.blocks {
        match block {
            Block::Heading { level, runs, .. } => {
                content_xml.push_str(&format!(r#"      <text:h text:outline-level="{level}">"#));
                for run in runs {
                    content_xml.push_str(&xml_escape(run.text()));
                }
                content_xml.push_str("</text:h>\n");
            }
            Block::Paragraph { runs, .. } => {
                content_xml.push_str("      <text:p>");
                for run in runs {
                    content_xml.push_str(&xml_escape(run.text()));
                }
                content_xml.push_str("</text:p>\n");
            }
            Block::List { items, .. } => {
                content_xml.push_str("      <text:list>\n");
                for item in items {
                    content_xml.push_str("        <text:list-item><text:p>");
                    for run in &item.runs {
                        content_xml.push_str(&xml_escape(run.text()));
                    }
                    content_xml.push_str("</text:p></text:list-item>\n");
                }
                content_xml.push_str("      </text:list>\n");
            }
            Block::Table { headers, rows, .. } => {
                content_xml.push_str("      <table:table>\n");
                // Header row
                content_xml.push_str("        <table:table-row>\n");
                for h in headers {
                    content_xml.push_str(&format!(
                        r#"          <table:table-cell><text:p>{}</text:p></table:table-cell>
"#,
                        xml_escape(h)
                    ));
                }
                content_xml.push_str("        </table:table-row>\n");
                // Data rows
                for row in rows {
                    content_xml.push_str("        <table:table-row>\n");
                    for cell in row {
                        content_xml.push_str(&format!(
                            r#"          <table:table-cell><text:p>{}</text:p></table:table-cell>
"#,
                            xml_escape(cell)
                        ));
                    }
                    content_xml.push_str("        </table:table-row>\n");
                }
                content_xml.push_str("      </table:table>\n");
            }
            Block::Code { text, .. } => {
                content_xml.push_str(&format!(
                    r#"      <text:p>{}</text:p>
"#,
                    xml_escape(text)
                ));
            }
            Block::PageBreak { .. } => {}
            Block::Unsupported { description, .. } => {
                content_xml.push_str(&format!(
                    r#"      <text:p>[{}]</text:p>
"#,
                    xml_escape(description)
                ));
            }
        }
    }

    content_xml.push_str(
        r#"    </office:text>
  </office:body>
</office:document-content>"#,
    );

    zip.write_all(content_xml.as_bytes())?;
    zip.finish()?;

    Ok(cursor.into_inner())
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

fn atomic_write_file(target_path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let temp_name = format!(
        ".tmp.export.{}.{}",
        target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("export"),
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
    use crate::document::{DocumentMetadata, DocumentSource, SourceAnchor};

    fn sample_document() -> Document {
        let blocks = vec![
            Block::Heading {
                level: 1,
                runs: vec![InlineRun::Text("Quarterly Report".to_string())],
                anchor: SourceAnchor::default(),
            },
            Block::Paragraph {
                runs: vec![
                    InlineRun::Text("Revenue exceeded expectations with ".to_string()),
                    InlineRun::Strong("15% growth".to_string()),
                    InlineRun::Text(" across regions.".to_string()),
                ],
                anchor: SourceAnchor::default(),
            },
            Block::Table {
                headers: vec!["Region".to_string(), "Revenue".to_string()],
                rows: vec![
                    vec!["North".to_string(), "$10M".to_string()],
                    vec!["South".to_string(), "$12M".to_string()],
                ],
                anchor: SourceAnchor::default(),
            },
        ];

        Document::new(
            DocumentSource::from_bytes("sample.txt", DocumentFormat::PlainText, b"fake"),
            DocumentMetadata {
                title: Some("Quarterly Report".to_string()),
                ..Default::default()
            },
            blocks,
        )
    }

    #[test]
    fn render_markdown_produces_valid_syntax() {
        let doc = sample_document();
        let md = render_markdown(&doc);

        assert!(md.contains("# Quarterly Report"));
        assert!(md.contains("**15% growth**"));
        assert!(md.contains("| Region | Revenue |"));
        assert!(md.contains("| North | $10M |"));
    }

    #[test]
    fn render_rtf_produces_rtf_header_and_content() {
        let doc = sample_document();
        let rtf_bytes = render_rtf(&doc);
        let rtf_str = String::from_utf8_lossy(&rtf_bytes);

        assert!(rtf_str.starts_with(r"{\rtf1\ansi"));
        assert!(rtf_str.contains("Quarterly Report"));
        assert!(rtf_str.contains(r"\b 15% growth\b0"));
        assert!(rtf_str.contains(r"\intbl\b Region\b0\cell"));
    }

    #[test]
    fn render_docx_produces_valid_zip_with_expected_entries() {
        let doc = sample_document();
        let docx_bytes = render_docx(&doc).unwrap();

        let reader = Cursor::new(docx_bytes);
        let mut zip = zip::ZipArchive::new(reader).unwrap();

        assert!(zip.by_name("[Content_Types].xml").is_ok());
        assert!(zip.by_name("_rels/.rels").is_ok());
        assert!(zip.by_name("docProps/core.xml").is_ok());
        assert!(zip.by_name("word/document.xml").is_ok());

        let mut doc_file = zip.by_name("word/document.xml").unwrap();
        let mut text = String::new();
        io::Read::read_to_string(&mut doc_file, &mut text).unwrap();

        assert!(text.contains("Quarterly Report"));
        assert!(text.contains("<w:b/>"));
        assert!(text.contains("15% growth"));
        assert!(text.contains("<w:tbl>"));
    }

    #[test]
    fn render_odt_produces_valid_zip_with_mimetype_and_content() {
        let doc = sample_document();
        let odt_bytes = render_odt(&doc).unwrap();

        let reader = Cursor::new(odt_bytes);
        let mut zip = zip::ZipArchive::new(reader).unwrap();

        assert!(zip.by_name("mimetype").is_ok());
        assert!(zip.by_name("META-INF/manifest.xml").is_ok());
        assert!(zip.by_name("meta.xml").is_ok());
        assert!(zip.by_name("content.xml").is_ok());

        let mut content_file = zip.by_name("content.xml").unwrap();
        let mut text = String::new();
        io::Read::read_to_string(&mut content_file, &mut text).unwrap();

        assert!(text.contains("Quarterly Report"));
        assert!(text.contains("15% growth"));
        assert!(text.contains("<table:table>"));
    }

    #[test]
    fn export_refuses_binary_source_overwrite() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(temp_dir.path()).unwrap();

        // Create a fake binary PDF file
        let pdf_path = temp_dir.path().join("spec.pdf");
        fs::write(&pdf_path, b"%PDF-1.4\n...fake content...").unwrap();

        let doc = sample_document();
        let res = export_document(
            &ws,
            &doc,
            ExportFormat::Markdown,
            Path::new("spec.pdf"),
            false, // allow_overwrite = false
        );

        assert!(matches!(
            res,
            Err(ExportError::BinarySourceOverwriteDenied { .. })
        ));
    }

    #[test]
    fn export_end_to_end_markdown_and_docx() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(temp_dir.path()).unwrap();
        let doc = sample_document();

        // 1. Export Markdown
        let report_md = export_document(
            &ws,
            &doc,
            ExportFormat::Markdown,
            Path::new("out.md"),
            false,
        )
        .unwrap();

        assert_eq!(report_md.format, ExportFormat::Markdown);
        assert!(temp_dir.path().join("out.md").exists());
        let md_content = fs::read_to_string(temp_dir.path().join("out.md")).unwrap();
        assert!(md_content.contains("# Quarterly Report"));

        // 2. Export DOCX
        let report_docx =
            export_document(&ws, &doc, ExportFormat::Docx, Path::new("out.docx"), false).unwrap();

        assert_eq!(report_docx.format, ExportFormat::Docx);
        assert!(temp_dir.path().join("out.docx").exists());
        assert!(report_docx.byte_size > 0);
    }
}
