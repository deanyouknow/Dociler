//! Native Rich Text Format (.rtf) document extractor.
//!
//! Parses RTF control words, character escapes, Unicode sequences, and formatting
//! into canonical AST blocks while skipping font tables, stylesheets, and binary pictures.

use crate::document::{
    Block, Document, DocumentFormat, DocumentMetadata, DocumentSource, InlineRun, SourceAnchor,
};
use crate::extractor::{ExtractionLimits, ExtractorError};

#[derive(Debug, Clone, Default)]
struct RtfGroupState {
    is_destination: bool,
    is_bold: bool,
    is_italic: bool,
    unicode_skip: usize,
    info_field: Option<String>,
}

pub fn extract_rtf(
    bytes: &[u8],
    display_name: &str,
    limits: &ExtractionLimits,
) -> Result<Document, ExtractorError> {
    DocumentFormat::Rtf
        .validate_bytes(bytes)
        .map_err(ExtractorError::Format)?;

    let source = DocumentSource::from_bytes(display_name, DocumentFormat::Rtf, bytes);
    let mut metadata = DocumentMetadata::default();
    let mut blocks = Vec::new();
    let mut block_index = 0;
    let mut total_text_bytes: u64 = 0;

    let mut group_stack: Vec<RtfGroupState> = vec![RtfGroupState {
        is_destination: false,
        is_bold: false,
        is_italic: false,
        unicode_skip: 1,
        info_field: None,
    }];

    let mut current_runs: Vec<InlineRun> = Vec::new();
    let mut pending_text = String::new();
    let mut current_bold = false;
    let mut current_italic = false;

    let flush_pending_run =
        |pending: &mut String, runs: &mut Vec<InlineRun>, bold: bool, italic: bool| {
            if !pending.is_empty() {
                let text = std::mem::take(pending);
                let run = if bold {
                    InlineRun::Strong(text)
                } else if italic {
                    InlineRun::Emphasis(text)
                } else {
                    InlineRun::Text(text)
                };
                runs.push(run);
            }
        };

    let mut idx = 0;
    while idx < bytes.len() {
        let b = bytes[idx];
        match b {
            b'{' => {
                let parent_state = group_stack.last().cloned().unwrap_or_default();
                group_stack.push(RtfGroupState {
                    is_destination: parent_state.is_destination,
                    is_bold: parent_state.is_bold,
                    is_italic: parent_state.is_italic,
                    unicode_skip: parent_state.unicode_skip,
                    info_field: None,
                });
                idx += 1;
            }
            b'}' => {
                flush_pending_run(
                    &mut pending_text,
                    &mut current_runs,
                    current_bold,
                    current_italic,
                );
                if group_stack.len() > 1 {
                    group_stack.pop();
                }
                if let Some(state) = group_stack.last() {
                    current_bold = state.is_bold;
                    current_italic = state.is_italic;
                }
                idx += 1;
            }
            b'\\' => {
                idx += 1;
                if idx >= bytes.len() {
                    break;
                }
                let next_b = bytes[idx];
                if next_b == b'\\' || next_b == b'{' || next_b == b'}' {
                    // Escaped control characters
                    let cur_dest = group_stack
                        .last()
                        .map(|s| s.is_destination)
                        .unwrap_or(false);
                    if !cur_dest {
                        pending_text.push(next_b as char);
                    }
                    idx += 1;
                    continue;
                }
                if next_b == b'\'' {
                    // Hex escape: \'xx
                    idx += 1;
                    if idx + 2 <= bytes.len() {
                        let hex_slice = &bytes[idx..idx + 2];
                        if let Ok(hex_str) = std::str::from_utf8(hex_slice) {
                            if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                                let cur_dest = group_stack
                                    .last()
                                    .map(|s| s.is_destination)
                                    .unwrap_or(false);
                                if !cur_dest {
                                    // Default Windows-1252 / Latin-1 decode
                                    let ch = if val < 128 {
                                        val as char
                                    } else {
                                        cp1252_to_char(val)
                                    };
                                    pending_text.push(ch);
                                }
                            }
                        }
                        idx += 2;
                    }
                    continue;
                }

                // Control word: alphabet followed by optional parameter
                let start_cw = idx;
                while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
                    idx += 1;
                }
                let word_slice = &bytes[start_cw..idx];
                let word = std::str::from_utf8(word_slice).unwrap_or("");

                // Optional integer parameter (can be negative)
                let start_param = idx;
                if idx < bytes.len() && (bytes[idx] == b'-' || bytes[idx].is_ascii_digit()) {
                    idx += 1;
                    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                        idx += 1;
                    }
                }
                let param_str = std::str::from_utf8(&bytes[start_param..idx]).unwrap_or("");
                let param = param_str.parse::<i32>().ok();

                // Trailing space after control word is consumed as delimiter
                if idx < bytes.len() && bytes[idx] == b' ' {
                    idx += 1;
                }

                // Handle control word
                let cur_state = group_stack.last_mut();
                match word {
                    "*" => {
                        if let Some(state) = cur_state {
                            state.is_destination = true;
                        }
                    }
                    "fonttbl" | "colortbl" | "stylesheet" | "pict" | "object" | "header"
                    | "footer" => {
                        if let Some(state) = cur_state {
                            state.is_destination = true;
                        }
                    }
                    "info" => {
                        // Keep processing info group
                    }
                    "title" => {
                        if let Some(state) = cur_state {
                            state.info_field = Some("title".to_string());
                        }
                    }
                    "author" => {
                        if let Some(state) = cur_state {
                            state.info_field = Some("author".to_string());
                        }
                    }
                    "b" => {
                        flush_pending_run(
                            &mut pending_text,
                            &mut current_runs,
                            current_bold,
                            current_italic,
                        );
                        let bold = param.map(|p| p != 0).unwrap_or(true);
                        current_bold = bold;
                        if let Some(state) = cur_state {
                            state.is_bold = bold;
                        }
                    }
                    "i" => {
                        flush_pending_run(
                            &mut pending_text,
                            &mut current_runs,
                            current_bold,
                            current_italic,
                        );
                        let italic = param.map(|p| p != 0).unwrap_or(true);
                        current_italic = italic;
                        if let Some(state) = cur_state {
                            state.is_italic = italic;
                        }
                    }
                    "par" => {
                        flush_pending_run(
                            &mut pending_text,
                            &mut current_runs,
                            current_bold,
                            current_italic,
                        );
                        if !current_runs.is_empty() {
                            blocks.push(Block::Paragraph {
                                runs: std::mem::take(&mut current_runs),
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
                    "line" => {
                        pending_text.push('\n');
                    }
                    "tab" => {
                        pending_text.push('\t');
                    }
                    "u" => {
                        // Unicode character: \uN
                        if let Some(code) = param {
                            let ucode = if code < 0 {
                                (code + 65536) as u32
                            } else {
                                code as u32
                            };
                            let ch = char::from_u32(ucode).unwrap_or('\u{FFFD}');
                            let cur_dest = group_stack
                                .last()
                                .map(|s| s.is_destination)
                                .unwrap_or(false);
                            if !cur_dest {
                                pending_text.push(ch);
                            }
                            // Skip replacement chars according to unicode_skip
                            let skip_count =
                                group_stack.last().map(|s| s.unicode_skip).unwrap_or(1);
                            for _ in 0..skip_count {
                                if idx < bytes.len()
                                    && bytes[idx] != b'\\'
                                    && bytes[idx] != b'{'
                                    && bytes[idx] != b'}'
                                {
                                    idx += 1;
                                }
                            }
                        }
                    }
                    "uc" => {
                        if let Some(p) = param {
                            if let Some(state) = cur_state {
                                state.unicode_skip = (p.max(0)) as usize;
                            }
                        }
                    }
                    _ => {}
                }
            }
            b'\r' | b'\n' => {
                // Ignore raw newlines in RTF
                idx += 1;
            }
            _ => {
                let cur_state = group_stack.last();
                let is_dest = cur_state.map(|s| s.is_destination).unwrap_or(false);
                let info_field = cur_state.and_then(|s| s.info_field.as_deref());

                if let Some(field) = info_field {
                    match field {
                        "title" => {
                            if metadata.title.is_none() {
                                metadata.title = Some(String::new());
                            }
                            metadata.title.as_mut().unwrap().push(b as char);
                        }
                        "author" => {
                            if metadata.author.is_none() {
                                metadata.author = Some(String::new());
                            }
                            metadata.author.as_mut().unwrap().push(b as char);
                        }
                        _ => {}
                    }
                } else if !is_dest {
                    total_text_bytes = total_text_bytes.saturating_add(1);
                    if total_text_bytes > limits.max_text_bytes {
                        return Err(ExtractorError::TextLimitExceeded {
                            limit_bytes: limits.max_text_bytes,
                            observed_bytes: total_text_bytes,
                        });
                    }
                    pending_text.push(b as char);
                }
                idx += 1;
            }
        }
    }

    flush_pending_run(
        &mut pending_text,
        &mut current_runs,
        current_bold,
        current_italic,
    );
    if !current_runs.is_empty() {
        blocks.push(Block::Paragraph {
            runs: current_runs,
            anchor: SourceAnchor {
                page: None,
                section: None,
                block_index,
                line_range: None,
            },
        });
    }

    // Clean metadata whitespace
    if let Some(t) = &mut metadata.title {
        *t = t.trim().to_string();
    }
    if let Some(a) = &mut metadata.author {
        *a = a.trim().to_string();
    }

    Ok(Document::new(source, metadata, blocks))
}

fn cp1252_to_char(byte: u8) -> char {
    match byte {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        _ => byte as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_rtf() {
        let rtf_data = br#"{\rtf1\ansi\deff0
{\fonttbl{\f0 Arial;}}
{\info{\title My RTF Title}{\author Jane Doe}}
\f0\fs24 Hello, this is \b bold\b0  and \i italic\i0  text.\par
Second paragraph with \'e9 accented character.\par
}"#;

        let limits = ExtractionLimits::default();
        let doc = extract_rtf(rtf_data, "test.rtf", &limits).expect("extract rtf");

        assert_eq!(doc.metadata.title.as_deref(), Some("My RTF Title"));
        assert_eq!(doc.metadata.author.as_deref(), Some("Jane Doe"));
        assert_eq!(doc.blocks.len(), 2);

        match &doc.blocks[0] {
            Block::Paragraph { runs, .. } => {
                assert!(
                    runs.iter()
                        .any(|r| r == &InlineRun::Strong("bold".to_string()))
                );
                assert!(
                    runs.iter()
                        .any(|r| r == &InlineRun::Emphasis("italic".to_string()))
                );
            }
            _ => panic!("expected paragraph block"),
        }

        match &doc.blocks[1] {
            Block::Paragraph { runs, .. } => {
                let full = runs.iter().map(|r| r.text()).collect::<String>();
                assert!(full.contains("accented character"));
                assert!(full.contains('é'));
            }
            _ => panic!("expected paragraph block"),
        }
    }
}
