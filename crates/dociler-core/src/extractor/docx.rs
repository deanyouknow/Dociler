//! Native Word (.docx) document extractor.
//!
//! Parses Office OpenXML documents from ZIP containers into canonical AST blocks
//! while strictly enforcing decompression and text limits.

use std::io::{Cursor, Read};

use crate::document::{
    Block, Document, DocumentFormat, DocumentMetadata, DocumentSource, InlineRun, SourceAnchor,
};
use crate::extractor::xml::{XmlScanner, XmlToken};
use crate::extractor::{ExtractionLimits, ExtractorError};

pub fn extract_docx(
    bytes: &[u8],
    display_name: &str,
    limits: &ExtractionLimits,
) -> Result<Document, ExtractorError> {
    DocumentFormat::Docx
        .validate_bytes(bytes)
        .map_err(ExtractorError::Format)?;

    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| ExtractorError::CorruptedDocument(format!("invalid ZIP container: {e}")))?;

    // Decompression bomb check: sum declared uncompressed sizes
    let mut total_uncompressed: u64 = 0;
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            total_uncompressed = total_uncompressed.saturating_add(file.size());
            if total_uncompressed > limits.max_decompressed_bytes {
                return Err(ExtractorError::DecompressionLimitExceeded {
                    limit_bytes: limits.max_decompressed_bytes,
                    observed_bytes: total_uncompressed,
                });
            }
        }
    }

    // Extract Dublin Core metadata from docProps/core.xml if present
    let metadata = extract_docx_metadata(&mut archive).unwrap_or_default();

    // Extract body from word/document.xml
    let document_xml_file = archive.by_name("word/document.xml").map_err(|_| {
        ExtractorError::CorruptedDocument("missing word/document.xml in DOCX archive".to_string())
    })?;

    let mut xml_string = String::new();
    document_xml_file
        .take(limits.max_decompressed_bytes)
        .read_to_string(&mut xml_string)
        .map_err(|e| {
            ExtractorError::CorruptedDocument(format!("failed reading document.xml: {e}"))
        })?;

    let source = DocumentSource::from_bytes(display_name, DocumentFormat::Docx, bytes);
    let mut blocks = Vec::new();
    let mut block_index = 0;
    let mut total_text_bytes: u64 = 0;

    let scanner = XmlScanner::new(&xml_string);

    // State machine tracking
    let mut in_p_pr = false;
    let mut p_style: Option<String> = None;
    let mut current_runs: Vec<InlineRun> = Vec::new();
    let mut in_r = false;
    let mut in_r_pr = false;
    let mut is_bold = false;
    let mut is_italic = false;
    let mut in_t = false;

    let mut in_tbl = false;
    let mut tbl_headers: Vec<String> = Vec::new();
    let mut tbl_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell_text = String::new();
    let mut in_tc = false;

    for token in scanner {
        match token {
            XmlToken::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                let local_name = name.split(':').last().unwrap_or(&name);
                match local_name {
                    "p" => {
                        p_style = None;
                        current_runs.clear();
                    }
                    "pPr" => {
                        in_p_pr = true;
                    }
                    "pStyle" if in_p_pr => {
                        for (k, v) in attributes {
                            if k.ends_with("val") {
                                p_style = Some(v);
                                break;
                            }
                        }
                    }
                    "r" => {
                        in_r = true;
                        is_bold = false;
                        is_italic = false;
                    }
                    "rPr" => {
                        in_r_pr = true;
                    }
                    "b" if in_r_pr => {
                        is_bold = true;
                    }
                    "i" if in_r_pr => {
                        is_italic = true;
                    }
                    "t" => {
                        in_t = true;
                    }
                    "tbl" => {
                        in_tbl = true;
                        tbl_headers.clear();
                        tbl_rows.clear();
                    }
                    "tr" if in_tbl => {
                        current_row.clear();
                    }
                    "tc" if in_tbl => {
                        in_tc = true;
                        current_cell_text.clear();
                    }
                    _ => {}
                }
                if self_closing {
                    match local_name {
                        "pPr" => in_p_pr = false,
                        "r" => in_r = false,
                        "rPr" => in_r_pr = false,
                        "t" => in_t = false,
                        "tc" => in_tc = false,
                        "tbl" => in_tbl = false,
                        _ => {}
                    }
                }
            }
            XmlToken::EndTag { name } => {
                let local_name = name.split(':').last().unwrap_or(&name);
                match local_name {
                    "p" => {
                        if in_tc {
                            // Inside a table cell
                            let p_text = current_runs
                                .iter()
                                .map(|r| r.text())
                                .collect::<Vec<_>>()
                                .join("");
                            if !current_cell_text.is_empty() && !p_text.is_empty() {
                                current_cell_text.push(' ');
                            }
                            current_cell_text.push_str(&p_text);
                        } else if !current_runs.is_empty() {
                            let heading_level = p_style.as_deref().and_then(parse_heading_level);
                            let anchor = SourceAnchor {
                                page: None,
                                section: None,
                                block_index,
                                line_range: None,
                            };
                            if let Some(level) = heading_level {
                                blocks.push(Block::Heading {
                                    level,
                                    runs: current_runs.clone(),
                                    anchor,
                                });
                            } else {
                                blocks.push(Block::Paragraph {
                                    runs: current_runs.clone(),
                                    anchor,
                                });
                            }
                            block_index += 1;
                        }
                        current_runs.clear();
                    }
                    "pPr" => in_p_pr = false,
                    "r" => in_r = false,
                    "rPr" => in_r_pr = false,
                    "t" => in_t = false,
                    "tc" if in_tbl => {
                        in_tc = false;
                        current_row.push(current_cell_text.trim().to_string());
                        current_cell_text.clear();
                    }
                    "tr" if in_tbl => {
                        if tbl_headers.is_empty() {
                            tbl_headers = current_row.clone();
                        } else {
                            tbl_rows.push(current_row.clone());
                        }
                        current_row.clear();
                    }
                    "tbl" => {
                        in_tbl = false;
                        if !tbl_headers.is_empty() || !tbl_rows.is_empty() {
                            blocks.push(Block::Table {
                                headers: tbl_headers.clone(),
                                rows: tbl_rows.clone(),
                                anchor: SourceAnchor {
                                    page: None,
                                    section: None,
                                    block_index,
                                    line_range: None,
                                },
                            });
                            block_index += 1;
                        }
                        tbl_headers.clear();
                        tbl_rows.clear();
                    }
                    _ => {}
                }
            }
            XmlToken::Text(text) => {
                if in_t && in_r {
                    total_text_bytes = total_text_bytes.saturating_add(text.len() as u64);
                    if total_text_bytes > limits.max_text_bytes {
                        return Err(ExtractorError::TextLimitExceeded {
                            limit_bytes: limits.max_text_bytes,
                            observed_bytes: total_text_bytes,
                        });
                    }

                    let run = if is_bold {
                        InlineRun::Strong(text)
                    } else if is_italic {
                        InlineRun::Emphasis(text)
                    } else {
                        InlineRun::Text(text)
                    };
                    current_runs.push(run);
                }
            }
        }
    }

    Ok(Document::new(source, metadata, blocks))
}

fn parse_heading_level(style: &str) -> Option<u8> {
    let lower = style.to_ascii_lowercase();
    if lower == "title" {
        return Some(1);
    }
    if lower == "subtitle" {
        return Some(2);
    }
    if let Some(rest) = lower
        .strip_prefix("heading")
        .or_else(|| lower.strip_prefix("heading "))
    {
        if let Ok(lvl) = rest.trim().parse::<u8>() {
            return Some(lvl.clamp(1, 6));
        }
    }
    None
}

fn extract_docx_metadata(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Option<DocumentMetadata> {
    let mut file = archive.by_name("docProps/core.xml").ok()?;
    let mut xml = String::new();
    file.read_to_string(&mut xml).ok()?;

    let mut meta = DocumentMetadata::default();
    let scanner = XmlScanner::new(&xml);
    let mut current_field = None;

    for token in scanner {
        match token {
            XmlToken::StartTag { name, .. } => {
                let local = name.split(':').last().unwrap_or(&name);
                match local {
                    "title" => current_field = Some("title"),
                    "creator" => current_field = Some("creator"),
                    "created" => current_field = Some("created"),
                    "modified" => current_field = Some("modified"),
                    _ => current_field = None,
                }
            }
            XmlToken::EndTag { .. } => {
                current_field = None;
            }
            XmlToken::Text(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    match current_field {
                        Some("title") => meta.title = Some(trimmed.to_string()),
                        Some("creator") => meta.author = Some(trimmed.to_string()),
                        Some("created") => meta.created = Some(trimmed.to_string()),
                        Some("modified") => meta.modified = Some(trimmed.to_string()),
                        _ => {}
                    }
                }
            }
        }
    }

    Some(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn build_test_docx(doc_xml: &str, core_xml: Option<&str>) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

            writer.start_file("word/document.xml", options).unwrap();
            writer.write_all(doc_xml.as_bytes()).unwrap();

            if let Some(core) = core_xml {
                writer.start_file("docProps/core.xml", options).unwrap();
                writer.write_all(core.as_bytes()).unwrap();
            }

            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_simple_docx() {
        let doc_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:t>Project Report</w:t></w:r>
    </w:p>
    <w:p>
      <w:r><w:t>This is the first paragraph with </w:t></w:r>
      <w:r><w:rPr><w:b/></w:rPr><w:t>bold text</w:t></w:r>
      <w:r><w:t>.</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let core_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>Sample DOCX</dc:title>
  <dc:creator>Test Author</dc:creator>
</cp:coreProperties>"#;

        let bytes = build_test_docx(doc_xml, Some(core_xml));
        let limits = ExtractionLimits::default();
        let doc = extract_docx(&bytes, "test.docx", &limits).expect("extract docx");

        assert_eq!(doc.metadata.title.as_deref(), Some("Sample DOCX"));
        assert_eq!(doc.metadata.author.as_deref(), Some("Test Author"));
        assert_eq!(doc.blocks.len(), 2);

        match &doc.blocks[0] {
            Block::Heading { level, runs, .. } => {
                assert_eq!(*level, 1);
                assert_eq!(runs[0].text(), "Project Report");
            }
            _ => panic!("expected heading block"),
        }

        match &doc.blocks[1] {
            Block::Paragraph { runs, .. } => {
                assert_eq!(runs.len(), 3);
                assert_eq!(runs[1], InlineRun::Strong("bold text".to_string()));
            }
            _ => panic!("expected paragraph block"),
        }
    }
}
