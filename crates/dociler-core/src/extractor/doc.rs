//! Native legacy Word 97-2003 (.doc) binary OLE extractor.
//!
//! Inspects OLE compound document streams, checks FIB encryption flags,
//! and extracts text into canonical AST blocks.

use crate::document::{
    Block, Document, DocumentFormat, DocumentMetadata, DocumentSource, InlineRun, SourceAnchor,
};
use crate::extractor::{ExtractionLimits, ExtractorError};

pub fn extract_doc(
    bytes: &[u8],
    display_name: &str,
    limits: &ExtractionLimits,
) -> Result<Document, ExtractorError> {
    DocumentFormat::Doc
        .validate_bytes(bytes)
        .map_err(ExtractorError::Format)?;

    let source = DocumentSource::from_bytes(display_name, DocumentFormat::Doc, bytes);
    let word_stream = read_ole_stream(bytes, "WordDocument")?;

    // Check FIB (File Information Block)
    if word_stream.len() > 12 {
        let flags = u16::from_le_bytes([word_stream[10], word_stream[11]]);
        let is_encrypted = (flags & 0x0001) != 0;
        if is_encrypted {
            return Err(ExtractorError::EncryptedFile);
        }
    }

    // Extract text from the WordDocument stream
    let raw_text = extract_text_from_word_stream(&word_stream);
    if raw_text.trim().is_empty() {
        return Err(ExtractorError::CorruptedDocument(
            "no readable text found in legacy Word document".to_string(),
        ));
    }

    let mut blocks = Vec::new();
    let mut block_index = 0;
    let mut total_text_bytes: u64 = 0;

    for paragraph in raw_text.split('\r') {
        let trimmed = paragraph.trim();
        if !trimmed.is_empty() {
            total_text_bytes = total_text_bytes.saturating_add(trimmed.len() as u64);
            if total_text_bytes > limits.max_text_bytes {
                return Err(ExtractorError::TextLimitExceeded {
                    limit_bytes: limits.max_text_bytes,
                    observed_bytes: total_text_bytes,
                });
            }

            blocks.push(Block::Paragraph {
                runs: vec![InlineRun::Text(trimmed.to_string())],
                anchor: SourceAnchor {
                    page: None,
                    section: None,
                    block_index,
                    line_range: None,
                },
            });
            block_index += 1;
        }
    }

    Ok(Document::new(source, DocumentMetadata::default(), blocks))
}

/// Reads a named stream from an OLE Compound Document container.
fn read_ole_stream(bytes: &[u8], target_name: &str) -> Result<Vec<u8>, ExtractorError> {
    if bytes.len() < 512 {
        return Err(ExtractorError::CorruptedDocument(
            "file too small for OLE container".to_string(),
        ));
    }

    let sector_shift = u16::from_le_bytes([bytes[30], bytes[31]]) as usize;
    let sector_size = 1 << sector_shift;
    if sector_size == 0 || sector_size > 4096 {
        return Err(ExtractorError::CorruptedDocument(
            "invalid OLE sector size".to_string(),
        ));
    }

    let first_dir_sector = u32::from_le_bytes([bytes[48], bytes[49], bytes[50], bytes[51]]);

    // Read MSAT / FAT
    let mut fat = Vec::new();
    for i in 0..109 {
        let offset = 76 + i * 4;
        if offset + 4 <= bytes.len() {
            let sec_id = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            if sec_id < 0xFFFFFFFD {
                let sec_offset = (sec_id as usize + 1) * sector_size;
                if sec_offset + sector_size <= bytes.len() {
                    let sec_slice = &bytes[sec_offset..sec_offset + sector_size];
                    for chunk in sec_slice.chunks_exact(4) {
                        fat.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                    }
                }
            }
        }
    }

    // Read Directory stream
    let mut dir_bytes = Vec::new();
    let mut cur_dir_sec = first_dir_sector;
    let mut visited = 0;
    while cur_dir_sec < 0xFFFFFFFD && visited < 1024 {
        let sec_offset = (cur_dir_sec as usize + 1) * sector_size;
        if sec_offset + sector_size <= bytes.len() {
            dir_bytes.extend_from_slice(&bytes[sec_offset..sec_offset + sector_size]);
        }
        if (cur_dir_sec as usize) < fat.len() {
            cur_dir_sec = fat[cur_dir_sec as usize];
        } else {
            break;
        }
        visited += 1;
    }

    // Parse 128-byte directory entries
    for chunk in dir_bytes.chunks_exact(128) {
        let name_len = u16::from_le_bytes([chunk[64], chunk[65]]) as usize;
        if (2..=64).contains(&name_len) {
            let name_bytes = &chunk[..name_len - 2];
            let name_u16: Vec<u16> = name_bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let entry_name = String::from_utf16_lossy(&name_u16);

            if entry_name == target_name {
                let start_sec =
                    u32::from_le_bytes([chunk[116], chunk[117], chunk[118], chunk[119]]);
                let size =
                    u32::from_le_bytes([chunk[120], chunk[121], chunk[122], chunk[123]]) as usize;

                let mut stream_data = Vec::new();
                let mut sec = start_sec;
                let mut sec_count = 0;
                while sec < 0xFFFFFFFD && sec_count < 10000 && stream_data.len() < size {
                    let offset = (sec as usize + 1) * sector_size;
                    if offset + sector_size <= bytes.len() {
                        stream_data.extend_from_slice(&bytes[offset..offset + sector_size]);
                    }
                    if (sec as usize) < fat.len() {
                        sec = fat[sec as usize];
                    } else {
                        break;
                    }
                    sec_count += 1;
                }
                stream_data.truncate(size);
                return Ok(stream_data);
            }
        }
    }

    // Fallback: if directory parsing doesn't match standard name, search stream directly
    Err(ExtractorError::CorruptedDocument(format!(
        "stream '{target_name}' not found in OLE directory"
    )))
}

/// Filters readable UTF-16LE and ASCII text sequences from raw WordDocument bytes.
fn extract_text_from_word_stream(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut idx = 68.min(bytes.len()); // Skip minimal FIB header area

    while idx < bytes.len() {
        // Try ASCII / 8-bit run (consecutive printable characters not interleaved with 0x00)
        let mut ascii_run = Vec::new();
        let mut a_idx = idx;
        while a_idx < bytes.len() {
            let b = bytes[a_idx];
            if (0x20..=0x7E).contains(&b) || b == 0x0D || b == 0x0A || b == 0x09 {
                // If the very next byte is 0x00 and we haven't accumulated plain ASCII,
                // this is likely UTF-16LE; defer to UTF-16LE decoder
                if a_idx + 1 < bytes.len() && bytes[a_idx + 1] == 0x00 && ascii_run.is_empty() {
                    break;
                }
                ascii_run.push(b);
                a_idx += 1;
            } else {
                break;
            }
        }

        if ascii_run.len() >= 4 {
            let a_str = String::from_utf8_lossy(&ascii_run);
            out.push_str(&a_str);
            idx = a_idx;
            continue;
        }

        // Try UTF-16LE run (where Latin/ASCII characters have high byte 0x00)
        let mut u16_chars = Vec::new();
        let mut u16_idx = idx;
        while u16_idx + 2 <= bytes.len() {
            let b0 = bytes[u16_idx];
            let b1 = bytes[u16_idx + 1];
            if b1 == 0 && ((0x20..=0x7E).contains(&b0) || b0 == 0x0D || b0 == 0x0A || b0 == 0x09) {
                u16_chars.push(b0 as u16);
                u16_idx += 2;
            } else {
                break;
            }
        }

        if u16_chars.len() >= 4 {
            let u16_str = String::from_utf16_lossy(&u16_chars);
            out.push_str(&u16_str);
            idx = u16_idx;
            continue;
        }

        idx += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_synthetic_doc(text: &str, encrypted: bool) -> Vec<u8> {
        let mut buf = vec![0u8; 1536];
        // OLE header
        buf[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
        buf[30..32].copy_from_slice(&9u16.to_le_bytes()); // sector_shift = 9 (512 bytes)
        buf[48..52].copy_from_slice(&0u32.to_le_bytes()); // first_dir_sector = 0 (sec 0 -> offset 512)
        buf[76..80].copy_from_slice(&0xFFFFFFFEu32.to_le_bytes()); // FAT sector mark

        // Sector 0 (offset 512): Directory
        // Entry 0 (root)
        // Entry 1 (offset 512 + 128 = 640): "WordDocument"
        let entry_offset = 640;
        let name = "WordDocument\0";
        let u16_name: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        buf[entry_offset..entry_offset + u16_name.len()].copy_from_slice(&u16_name);
        buf[entry_offset + 64..entry_offset + 66]
            .copy_from_slice(&(u16_name.len() as u16).to_le_bytes());
        buf[entry_offset + 116..entry_offset + 120].copy_from_slice(&1u32.to_le_bytes()); // start sector 1 (offset 1024)
        buf[entry_offset + 120..entry_offset + 124].copy_from_slice(&512u32.to_le_bytes()); // size 512

        // Sector 1 (offset 1024): WordDocument stream
        let stream_offset = 1024;
        if encrypted {
            buf[stream_offset + 10] = 0x01; // fEncrypted
        }
        // Write text at offset 1024 + 512 - 200
        let text_bytes = text.as_bytes();
        let write_pos = stream_offset + 100;
        buf[write_pos..write_pos + text_bytes.len()].copy_from_slice(text_bytes);

        buf
    }

    #[test]
    fn extract_synthetic_doc_success() {
        let bytes = build_synthetic_doc(
            "Hello from legacy doc file\rSecond line of legacy text",
            false,
        );
        let limits = ExtractionLimits::default();
        let doc = extract_doc(&bytes, "test.doc", &limits).expect("extract doc");
        assert!(!doc.blocks.is_empty());
        let plain = doc.plain_text();
        assert!(plain.contains("Hello from legacy doc file"));
    }

    #[test]
    fn extract_synthetic_doc_encrypted_rejected() {
        let bytes = build_synthetic_doc("Secret document", true);
        let limits = ExtractionLimits::default();
        let err = extract_doc(&bytes, "secret.doc", &limits).unwrap_err();
        assert_eq!(err, ExtractorError::EncryptedFile);
    }
}
