//! Lightweight streaming XML tokenizer and entity decoder.
//!
//! Provides memory-bounded XML element scanning for `.docx` and `.odt`
//! parsing without external heavy dependencies.

/// A token emitted during XML scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlToken {
    StartTag {
        name: String,
        attributes: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    Text(String),
}

/// Decodes standard XML entities and numeric character references.
pub fn decode_xml_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '&' {
            let mut entity = String::with_capacity(10);
            let mut found_semicolon = false;
            for _ in 0..12 {
                if let Some(&next_c) = chars.peek() {
                    chars.next();
                    if next_c == ';' {
                        found_semicolon = true;
                        break;
                    }
                    entity.push(next_c);
                } else {
                    break;
                }
            }

            if found_semicolon {
                match entity.as_str() {
                    "amp" => out.push('&'),
                    "lt" => out.push('<'),
                    "gt" => out.push('>'),
                    "quot" => out.push('"'),
                    "apos" => out.push('\''),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        if let Ok(code) = u32::from_str_radix(&entity[2..], 16) {
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            } else {
                                out.push('\u{FFFD}');
                            }
                        } else {
                            out.push('&');
                            out.push_str(&entity);
                            out.push(';');
                        }
                    }
                    _ if entity.starts_with('#') => {
                        if let Ok(code) = entity[1..].parse::<u32>() {
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            } else {
                                out.push('\u{FFFD}');
                            }
                        } else {
                            out.push('&');
                            out.push_str(&entity);
                            out.push(';');
                        }
                    }
                    _ => {
                        out.push('&');
                        out.push_str(&entity);
                        out.push(';');
                    }
                }
            } else {
                out.push('&');
                out.push_str(&entity);
            }
        } else {
            out.push(c);
        }
    }

    out
}

/// Tokenizes an XML string into a stream of `XmlToken` items.
pub struct XmlScanner<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> XmlScanner<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }
}

impl Iterator for XmlScanner<'_> {
    type Item = XmlToken;

    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < self.input.len() {
            let rem = self.remaining();

            // Comment: <!-- ... -->
            if rem.starts_with("<!--") {
                if let Some(end) = rem.find("-->") {
                    self.pos += end + 3;
                    continue;
                } else {
                    self.pos = self.input.len();
                    return None;
                }
            }

            // Processing instruction: <? ... ?>
            if rem.starts_with("<?") {
                if let Some(end) = rem.find("?>") {
                    self.pos += end + 2;
                    continue;
                } else {
                    self.pos = self.input.len();
                    return None;
                }
            }

            // CDATA: <![CDATA[ ... ]]>
            if rem.starts_with("<![CDATA[") {
                if let Some(end) = rem.find("]]>") {
                    let cdata_content = &rem[9..end];
                    self.pos += end + 3;
                    return Some(XmlToken::Text(cdata_content.to_string()));
                } else {
                    self.pos = self.input.len();
                    return None;
                }
            }

            // Doctype / other declarations: <! ... >
            if rem.starts_with("<!") {
                if let Some(end) = rem.find('>') {
                    self.pos += end + 1;
                    continue;
                } else {
                    self.pos = self.input.len();
                    return None;
                }
            }

            // End tag: </tag>
            if rem.starts_with("</") {
                if let Some(end) = rem.find('>') {
                    let tag_name = rem[2..end].trim().to_string();
                    self.pos += end + 1;
                    return Some(XmlToken::EndTag { name: tag_name });
                } else {
                    self.pos = self.input.len();
                    return None;
                }
            }

            // Start tag or self-closing tag: <tag ...> or <tag .../>
            if rem.starts_with('<') {
                if let Some(end) = rem.find('>') {
                    let tag_body = &rem[1..end].trim();
                    let self_closing = tag_body.ends_with('/');
                    let clean_body = if self_closing {
                        tag_body.strip_suffix('/').unwrap_or(tag_body).trim()
                    } else {
                        tag_body
                    };

                    let (name, attributes) = parse_tag_body(clean_body);
                    self.pos += end + 1;
                    return Some(XmlToken::StartTag {
                        name,
                        attributes,
                        self_closing,
                    });
                } else {
                    self.pos = self.input.len();
                    return None;
                }
            }

            // Text up to next '<'
            if let Some(next_tag) = rem.find('<') {
                let text_chunk = &rem[..next_tag];
                self.pos += next_tag;
                let decoded = decode_xml_entities(text_chunk);
                return Some(XmlToken::Text(decoded));
            } else {
                let text_chunk = rem;
                self.pos = self.input.len();
                let decoded = decode_xml_entities(text_chunk);
                return Some(XmlToken::Text(decoded));
            }
        }

        None
    }
}

/// Parses the inside of a start tag (e.g. `w:pStyle w:val="Heading1"`) into name and attributes.
fn parse_tag_body(body: &str) -> (String, Vec<(String, String)>) {
    let mut parts = body.split_whitespace();
    let name = parts.next().unwrap_or("").to_string();
    let mut attributes = Vec::new();

    let mut remaining = body[name.len()..].trim();
    while !remaining.is_empty() {
        if let Some(eq_pos) = remaining.find('=') {
            let key = remaining[..eq_pos].trim().to_string();
            let after_eq = remaining[eq_pos + 1..].trim_start();
            if let Some(quote_char) = after_eq.chars().next() {
                if quote_char == '"' || quote_char == '\'' {
                    let val_start = 1;
                    if let Some(val_end) = after_eq[val_start..].find(quote_char) {
                        let val = &after_eq[val_start..val_start + val_end];
                        attributes.push((key, decode_xml_entities(val)));
                        remaining = after_eq[val_start + val_end + 1..].trim_start();
                        continue;
                    }
                }
            }
            // Fallback unquoted or broken attribute
            let val = after_eq.split_whitespace().next().unwrap_or("");
            attributes.push((key, decode_xml_entities(val)));
            remaining = after_eq[val.len()..].trim_start();
        } else {
            break;
        }
    }

    (name, attributes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_decoding() {
        assert_eq!(decode_xml_entities("Hello &amp; world"), "Hello & world");
        assert_eq!(decode_xml_entities("&lt;tag&gt;"), "<tag>");
        assert_eq!(decode_xml_entities("&quot;quoted&apos;"), "\"quoted'");
        assert_eq!(decode_xml_entities("&#65;&#66;&#67;"), "ABC");
        assert_eq!(decode_xml_entities("&#x41;&#x42;&#x43;"), "ABC");
    }

    #[test]
    fn xml_scanner_tokenization() {
        let xml = r#"<?xml version="1.0"?>
<!-- comment -->
<w:document xmlns:w="http://example.com">
  <w:p>
    <w:pPr>
      <w:pStyle w:val="Heading1"/>
    </w:pPr>
    <w:r>
      <w:t>Hello &amp; welcome</w:t>
    </w:r>
  </w:p>
</w:document>"#;

        let tokens: Vec<XmlToken> = XmlScanner::new(xml).collect();
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t, XmlToken::StartTag { name, .. } if name == "w:document"))
        );
        assert!(tokens.iter().any(|t| matches!(t, XmlToken::StartTag { name, self_closing: true, .. } if name == "w:pStyle")));
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t, XmlToken::Text(text) if text.contains("Hello & welcome")))
        );
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t, XmlToken::EndTag { name } if name == "w:document"))
        );
    }
}
