//! Native OpenDocument (.odt) document extractor.
//!
//! Parses OpenDocument XML from ZIP containers into canonical AST blocks
//! while strictly enforcing decompression and text limits.

use std::io::{Cursor, Read};

use crate::document::{
    Block, Document, DocumentFormat, DocumentMetadata, DocumentSource, InlineRun, ListItem,
    SourceAnchor,
};
use crate::extractor::xml::{XmlScanner, XmlToken};
use crate::extractor::{ExtractionLimits, ExtractorError};

pub fn extract_odt(
    bytes: &[u8],
    display_name: &str,
    limits: &ExtractionLimits,
) -> Result<Document, ExtractorError> {
    DocumentFormat::Odt
        .validate_bytes(bytes)
        .map_err(ExtractorError::Format)?;

    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| ExtractorError::CorruptedDocument(format!("invalid ZIP container: {e}")))?;

    // Decompression bomb check
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

    // Extract metadata from meta.xml if present
    let metadata = extract_odt_metadata(&mut archive).unwrap_or_default();

    // Read content.xml
    let content_xml_file = archive.by_name("content.xml").map_err(|_| {
        ExtractorError::CorruptedDocument("missing content.xml in ODT archive".to_string())
    })?;

    let mut xml_string = String::new();
    content_xml_file
        .take(limits.max_decompressed_bytes)
        .read_to_string(&mut xml_string)
        .map_err(|e| {
            ExtractorError::CorruptedDocument(format!("failed reading content.xml: {e}"))
        })?;

    let source = DocumentSource::from_bytes(display_name, DocumentFormat::Odt, bytes);
    let mut blocks = Vec::new();
    let mut block_index = 0;
    let mut total_text_bytes: u64 = 0;

    let scanner = XmlScanner::new(&xml_string);

    let mut in_h = false;
    let mut h_level: u8 = 1;
    let mut in_p = false;
    let mut current_runs: Vec<InlineRun> = Vec::new();
    let mut in_list = false;
    let mut list_items: Vec<ListItem> = Vec::new();
    let mut in_list_item = false;
    let mut tbl_headers: Vec<String> = Vec::new();
    let mut tbl_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell_text = String::new();
    let mut in_cell = false;

    for token in scanner {
        match token {
            XmlToken::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                let local_name = name.split(':').last().unwrap_or(&name);
                match local_name {
                    "h" => {
                        in_h = true;
                        h_level = 1;
                        for (k, v) in attributes {
                            if k.ends_with("outline-level") {
                                if let Ok(lvl) = v.parse::<u8>() {
                                    h_level = lvl.clamp(1, 6);
                                }
                            }
                        }
                        current_runs.clear();
                    }
                    "p" => {
                        in_p = true;
                        current_runs.clear();
                    }
                    "list" => {
                        in_list = true;
                        list_items.clear();
                    }
                    "list-item" => {
                        in_list_item = true;
                    }
                    "table" => {
                        tbl_headers.clear();
                        tbl_rows.clear();
                    }
                    "table-row" => {
                        current_row.clear();
                    }
                    "table-cell" => {
                        in_cell = true;
                        current_cell_text.clear();
                    }
                    _ => {}
                }
                if self_closing {
                    match local_name {
                        "h" => in_h = false,
                        "p" => in_p = false,
                        "list-item" => in_list_item = false,
                        "table-cell" => in_cell = false,
                        _ => {}
                    }
                }
            }
            XmlToken::EndTag { name } => {
                let local_name = name.split(':').last().unwrap_or(&name);
                match local_name {
                    "h" => {
                        in_h = false;
                        if !current_runs.is_empty() {
                            blocks.push(Block::Heading {
                                level: h_level,
                                runs: current_runs.clone(),
                                anchor: SourceAnchor {
                                    page: None,
                                    section: None,
                                    block_index,
                                    line_range: None,
                                },
                            });
                            block_index += 1;
                        }
                        current_runs.clear();
                    }
                    "p" => {
                        in_p = false;
                        let p_text = current_runs
                            .iter()
                            .map(|r| r.text())
                            .collect::<Vec<_>>()
                            .join("");

                        if in_cell {
                            if !current_cell_text.is_empty() && !p_text.is_empty() {
                                current_cell_text.push(' ');
                            }
                            current_cell_text.push_str(&p_text);
                        } else if in_list && in_list_item {
                            list_items.push(ListItem::new(current_runs.clone()));
                        } else if !current_runs.is_empty() {
                            blocks.push(Block::Paragraph {
                                runs: current_runs.clone(),
                                anchor: SourceAnchor {
                                    page: None,
                                    section: None,
                                    block_index,
                                    line_range: None,
                                },
                            });
                            block_index += 1;
                        }
                        current_runs.clear();
                    }
                    "list-item" => {
                        in_list_item = false;
                    }
                    "list" => {
                        in_list = false;
                        if !list_items.is_empty() {
                            blocks.push(Block::List {
                                ordered: false,
                                items: list_items.clone(),
                                anchor: SourceAnchor {
                                    page: None,
                                    section: None,
                                    block_index,
                                    line_range: None,
                                },
                            });
                            block_index += 1;
                            list_items.clear();
                        }
                    }
                    "table-cell" => {
                        in_cell = false;
                        current_row.push(current_cell_text.trim().to_string());
                        current_cell_text.clear();
                    }
                    "table-row" => {
                        if tbl_headers.is_empty() {
                            tbl_headers = current_row.clone();
                        } else {
                            tbl_rows.push(current_row.clone());
                        }
                        current_row.clear();
                    }
                    "table" => {
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
                if in_h || in_p {
                    total_text_bytes = total_text_bytes.saturating_add(text.len() as u64);
                    if total_text_bytes > limits.max_text_bytes {
                        return Err(ExtractorError::TextLimitExceeded {
                            limit_bytes: limits.max_text_bytes,
                            observed_bytes: total_text_bytes,
                        });
                    }
                    current_runs.push(InlineRun::Text(text));
                }
            }
        }
    }

    Ok(Document::new(source, metadata, blocks))
}

fn extract_odt_metadata(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Option<DocumentMetadata> {
    let mut file = archive.by_name("meta.xml").ok()?;
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
                    "date" => current_field = Some("date"),
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
                        Some("date") => meta.created = Some(trimmed.to_string()),
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

    fn build_test_odt(content_xml: &str, meta_xml: Option<&str>) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

            writer
                .start_file(
                    "mimetype",
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer
                .write_all(b"application/vnd.oasis.opendocument.text")
                .unwrap();

            writer.start_file("content.xml", options).unwrap();
            writer.write_all(content_xml.as_bytes()).unwrap();

            if let Some(meta) = meta_xml {
                writer.start_file("meta.xml", options).unwrap();
                writer.write_all(meta.as_bytes()).unwrap();
            }

            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_simple_odt() {
        let content_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
                         xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <office:body>
    <office:text>
      <text:h text:outline-level="1">OpenDocument Guide</text:h>
      <text:p>This is a paragraph in an ODT document.</text:p>
    </office:text>
  </office:body>
</office:document-content>"#;

        let meta_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
                      xmlns:dc="http://purl.org/dc/elements/1.1/">
  <office:meta>
    <dc:title>Test ODT Document</dc:title>
    <dc:creator>Dociler Author</dc:creator>
  </office:meta>
</office:document-meta>"#;

        let bytes = build_test_odt(content_xml, Some(meta_xml));
        let limits = ExtractionLimits::default();
        let doc = extract_odt(&bytes, "test.odt", &limits).expect("extract odt");

        assert_eq!(doc.metadata.title.as_deref(), Some("Test ODT Document"));
        assert_eq!(doc.metadata.author.as_deref(), Some("Dociler Author"));
        assert_eq!(doc.blocks.len(), 2);

        match &doc.blocks[0] {
            Block::Heading { level, runs, .. } => {
                assert_eq!(*level, 1);
                assert_eq!(runs[0].text(), "OpenDocument Guide");
            }
            _ => panic!("expected heading block"),
        }

        match &doc.blocks[1] {
            Block::Paragraph { runs, .. } => {
                assert_eq!(runs[0].text(), "This is a paragraph in an ODT document.");
            }
            _ => panic!("expected paragraph block"),
        }
    }
}
