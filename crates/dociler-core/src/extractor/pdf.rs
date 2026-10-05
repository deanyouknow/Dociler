//! Native Portable Document Format (.pdf) text extractor.
//!
//! Extracts embedded text streams from unencrypted PDFs, decompresses FlateDecode
//! streams using pure-Rust zlib, detects scanned/image-only PDFs with actionable errors,
//! and normalizes text into canonical AST blocks with page anchors.

use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::document::{
    Block, Document, DocumentFormat, DocumentMetadata, DocumentSource, InlineRun, SourceAnchor,
};
use crate::extractor::{ExtractionLimits, ExtractorError};

pub fn extract_pdf(
    bytes: &[u8],
    display_name: &str,
    limits: &ExtractionLimits,
) -> Result<Document, ExtractorError> {
    DocumentFormat::Pdf
        .validate_bytes(bytes)
        .map_err(ExtractorError::Format)?;

    let source = DocumentSource::from_bytes(display_name, DocumentFormat::Pdf, bytes);
    let metadata = extract_pdf_info(bytes).unwrap_or_default();

    let mut blocks = Vec::new();
    let mut block_index = 0;
    let mut total_text_bytes: u64 = 0;
    let mut total_characters_extracted = 0;
    let mut total_decompressed_bytes: u64 = 0;

    // Scan all streams in the PDF
    let streams = find_streams(bytes);
    let mut page_number: u32 = 1;

    for (dict_bytes, stream_data) in streams {
        let is_flate = dict_bytes.windows(12).any(|w| w == b"/FlateDecode");
        let decompressed = if is_flate {
            let mut decoder = ZlibDecoder::new(stream_data);
            let mut decomp = Vec::new();
            let mut take_reader = (&mut decoder).take(limits.max_decompressed_bytes + 1);
            if take_reader.read_to_end(&mut decomp).is_err() {
                // If standard zlib fails, try raw deflate
                decomp.clear();
                let mut raw_decoder = flate2::read::DeflateDecoder::new(stream_data);
                let _ = raw_decoder.read_to_end(&mut decomp);
            }
            total_decompressed_bytes = total_decompressed_bytes.saturating_add(decomp.len() as u64);
            if total_decompressed_bytes > limits.max_decompressed_bytes {
                return Err(ExtractorError::DecompressionLimitExceeded {
                    limit_bytes: limits.max_decompressed_bytes,
                    observed_bytes: total_decompressed_bytes,
                });
            }
            decomp
        } else {
            stream_data.to_vec()
        };

        let stream_text = extract_text_from_pdf_stream(&decompressed);
        if !stream_text.is_empty() {
            total_characters_extracted += stream_text.chars().count();
            total_text_bytes = total_text_bytes.saturating_add(stream_text.len() as u64);
            if total_text_bytes > limits.max_text_bytes {
                return Err(ExtractorError::TextLimitExceeded {
                    limit_bytes: limits.max_text_bytes,
                    observed_bytes: total_text_bytes,
                });
            }

            for paragraph in stream_text.split("\n\n") {
                let trimmed = paragraph.trim();
                if !trimmed.is_empty() {
                    blocks.push(Block::Paragraph {
                        runs: vec![InlineRun::Text(trimmed.to_string())],
                        anchor: SourceAnchor {
                            page: Some(page_number),
                            section: None,
                            block_index,
                            line_range: None,
                        },
                    });
                    block_index += 1;
                }
            }
            page_number += 1;
        }
    }

    if total_characters_extracted == 0 {
        return Err(ExtractorError::ScannedPdfNoText);
    }

    Ok(Document::new(source, metadata, blocks))
}

/// Locates `stream` ... `endstream` boundaries and their preceding dictionary slices.
fn find_streams(bytes: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut streams = Vec::new();
    let mut pos = 0;

    while pos < bytes.len() {
        if let Some(stream_rel) = bytes[pos..].windows(6).position(|w| w == b"stream") {
            let stream_kw_pos = pos + stream_rel;
            let dict_start = stream_kw_pos.saturating_sub(512);
            let dict_slice = &bytes[dict_start..stream_kw_pos];

            // Advance past "stream\r\n" or "stream\n"
            let mut data_start = stream_kw_pos + 6;
            if data_start < bytes.len() && bytes[data_start] == b'\r' {
                data_start += 1;
            }
            if data_start < bytes.len() && bytes[data_start] == b'\n' {
                data_start += 1;
            }

            if let Some(end_rel) = bytes[data_start..]
                .windows(9)
                .position(|w| w == b"endstream")
            {
                let data_end = data_start + end_rel;
                // Trim trailing \r or \n before endstream
                let mut clean_end = data_end;
                while clean_end > data_start
                    && (bytes[clean_end - 1] == b'\r' || bytes[clean_end - 1] == b'\n')
                {
                    clean_end -= 1;
                }
                streams.push((dict_slice, &bytes[data_start..clean_end]));
                pos = data_end + 9;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    streams
}

/// Extracts text inside `BT` (Begin Text) ... `ET` (End Text) blocks in a content stream.
fn extract_text_from_pdf_stream(stream: &[u8]) -> String {
    let mut out = String::new();
    let mut pos = 0;

    while pos < stream.len() {
        // Look for "BT"
        let mut found_bt = None;
        for (offset, w) in stream[pos..].windows(2).enumerate() {
            if w == b"BT" && is_token_delim(stream, pos + offset, 2) {
                found_bt = Some(offset);
                break;
            }
        }

        if let Some(bt_rel) = found_bt {
            let bt_pos = pos + bt_rel + 2;
            // Look for matching "ET"
            let mut found_et = None;
            for (offset, w) in stream[bt_pos..].windows(2).enumerate() {
                if w == b"ET" && is_token_delim(stream, bt_pos + offset, 2) {
                    found_et = Some(offset);
                    break;
                }
            }

            if let Some(et_rel) = found_et {
                let text_block = &stream[bt_pos..bt_pos + et_rel];
                let block_text = parse_text_operators(text_block);
                if !block_text.is_empty() {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&block_text);
                }
                pos = bt_pos + et_rel + 2;
            } else {
                pos = bt_pos;
            }
        } else {
            break;
        }
    }

    out
}

fn is_token_delim(slice: &[u8], token_pos: usize, token_len: usize) -> bool {
    let before_ok = if token_pos == 0 {
        true
    } else {
        slice[token_pos - 1].is_ascii_whitespace()
            || slice[token_pos - 1] == b'<'
            || slice[token_pos - 1] == b'/'
    };
    let after_pos = token_pos + token_len;
    let after_ok = if after_pos >= slice.len() {
        true
    } else {
        slice[after_pos].is_ascii_whitespace()
            || slice[after_pos] == b'>'
            || slice[after_pos] == b'/'
    };
    before_ok && after_ok
}

/// Parses Tj, TJ, ', " operators inside a BT ... ET block.
fn parse_text_operators(block: &[u8]) -> String {
    let mut out = String::new();
    let mut idx = 0;

    while idx < block.len() {
        let b = block[idx];

        // String literal: ( ... )
        if b == b'(' {
            let (str_val, next_idx) = parse_pdf_literal_string(block, idx);
            idx = next_idx;

            // Look ahead for operator: Tj, ', "
            let op = peek_next_operator(block, idx);
            match op {
                Some("Tj") | Some("'") | Some("\"") => {
                    out.push_str(&str_val);
                    if op == Some("'") || op == Some("\"") {
                        out.push('\n');
                    }
                }
                _ => {}
            }
            continue;
        }

        // Hex string literal: < ... > (must not be << dictionary)
        if b == b'<' && idx + 1 < block.len() && block[idx + 1] != b'<' {
            let (str_val, next_idx) = parse_pdf_hex_string(block, idx);
            idx = next_idx;
            let op = peek_next_operator(block, idx);
            if matches!(op, Some("Tj") | Some("'") | Some("\"")) {
                out.push_str(&str_val);
            }
            continue;
        }

        // Array for TJ operator: [ (str) 20 (str) ] TJ
        if b == b'[' {
            let (arr_text, next_idx) = parse_pdf_tj_array(block, idx);
            idx = next_idx;
            let op = peek_next_operator(block, idx);
            if op == Some("TJ") {
                out.push_str(&arr_text);
            }
            continue;
        }

        // Line break operators: T*, TD, Td
        if b == b'T' && idx + 1 < block.len() && block[idx + 1] == b'*' {
            out.push('\n');
            idx += 2;
            continue;
        }

        idx += 1;
    }

    out.trim().to_string()
}

fn parse_pdf_literal_string(block: &[u8], start: usize) -> (String, usize) {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut idx = start;

    while idx < block.len() {
        let b = block[idx];
        if b == b'\\' {
            idx += 1;
            if idx < block.len() {
                let esc = block[idx];
                match esc {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0C),
                    b'(' => out.push(b'('),
                    b')' => out.push(b')'),
                    b'\\' => out.push(b'\\'),
                    b'0'..=b'7' => {
                        // Octal escape \ddd
                        let mut oct_val = (esc - b'0') as u32;
                        for _ in 0..2 {
                            if idx + 1 < block.len() && (b'0'..=b'7').contains(&block[idx + 1]) {
                                idx += 1;
                                oct_val = oct_val * 8 + (block[idx] - b'0') as u32;
                            } else {
                                break;
                            }
                        }
                        out.push(oct_val as u8);
                    }
                    _ => out.push(esc),
                }
                idx += 1;
            }
            continue;
        }
        if b == b'(' {
            depth += 1;
            if depth > 1 {
                out.push(b);
            }
            idx += 1;
            continue;
        }
        if b == b')' {
            depth -= 1;
            idx += 1;
            if depth == 0 {
                break;
            }
            out.push(b);
            continue;
        }
        out.push(b);
        idx += 1;
    }

    let text = String::from_utf8_lossy(&out).into_owned();
    (text, idx)
}

fn parse_pdf_hex_string(block: &[u8], start: usize) -> (String, usize) {
    let mut hex_digits = String::new();
    let mut idx = start + 1;

    while idx < block.len() {
        let b = block[idx];
        if b == b'>' {
            idx += 1;
            break;
        }
        if b.is_ascii_hexdigit() {
            hex_digits.push(b as char);
        }
        idx += 1;
    }

    if hex_digits.len() % 2 != 0 {
        hex_digits.push('0');
    }

    let mut bytes = Vec::new();
    for i in (0..hex_digits.len()).step_by(2) {
        if let Ok(val) = u8::from_str_radix(&hex_digits[i..i + 2], 16) {
            bytes.push(val);
        }
    }

    (String::from_utf8_lossy(&bytes).into_owned(), idx)
}

fn parse_pdf_tj_array(block: &[u8], start: usize) -> (String, usize) {
    let mut out = String::new();
    let mut idx = start + 1;

    while idx < block.len() {
        let b = block[idx];
        if b == b']' {
            idx += 1;
            break;
        }
        if b == b'(' {
            let (str_val, next) = parse_pdf_literal_string(block, idx);
            out.push_str(&str_val);
            idx = next;
            continue;
        }
        if b == b'<' {
            let (str_val, next) = parse_pdf_hex_string(block, idx);
            out.push_str(&str_val);
            idx = next;
            continue;
        }
        idx += 1;
    }

    (out, idx)
}

fn peek_next_operator(block: &[u8], mut idx: usize) -> Option<&'static str> {
    while idx < block.len() && block[idx].is_ascii_whitespace() {
        idx += 1;
    }
    if idx + 2 <= block.len() {
        let slice = &block[idx..idx + 2];
        if slice == b"Tj" {
            return Some("Tj");
        }
        if slice == b"TJ" {
            return Some("TJ");
        }
    }
    if idx < block.len() {
        if block[idx] == b'\'' {
            return Some("'");
        }
        if block[idx] == b'"' {
            return Some("\"");
        }
    }
    None
}

fn extract_pdf_info(bytes: &[u8]) -> Option<DocumentMetadata> {
    let mut meta = DocumentMetadata::default();
    if let Some(info_pos) = bytes.windows(5).position(|w| w == b"/Info") {
        let snippet = &bytes[info_pos..info_pos.saturating_add(1024).min(bytes.len())];
        if let Some(title) = find_pdf_doc_entry(snippet, b"/Title") {
            meta.title = Some(title);
        }
        if let Some(author) = find_pdf_doc_entry(snippet, b"/Author") {
            meta.author = Some(author);
        }
    }
    Some(meta)
}

fn find_pdf_doc_entry(snippet: &[u8], key: &[u8]) -> Option<String> {
    let key_pos = snippet.windows(key.len()).position(|w| w == key)?;
    let rem = &snippet[key_pos + key.len()..];
    let open_paren = rem.iter().position(|&b| b == b'(')?;
    let (val, _) = parse_pdf_literal_string(rem, open_paren);
    let trimmed = val.trim();
    if !trimmed.is_empty() {
        Some(trimmed.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_text_pdf() {
        let pdf_data = b"%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>
endobj
4 0 obj
<< /Length 50 >>
stream
BT
/F1 12 Tf
(Hello from Dociler PDF parser) Tj
ET
endstream
endobj
xref
0 5
trailer
<< /Root 1 0 R >>
%%EOF";

        let limits = ExtractionLimits::default();
        let doc = extract_pdf(pdf_data, "test.pdf", &limits).expect("extract pdf");

        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::Paragraph { runs, anchor } => {
                assert_eq!(runs[0].text(), "Hello from Dociler PDF parser");
                assert_eq!(anchor.page, Some(1));
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn extract_scanned_pdf_error() {
        let scanned_pdf = b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length 20 /Filter /FlateDecode >>
stream
endstream
endobj
%%EOF";

        let limits = ExtractionLimits::default();
        let err = extract_pdf(scanned_pdf, "scanned.pdf", &limits).unwrap_err();
        assert_eq!(err, ExtractorError::ScannedPdfNoText);
    }
}
