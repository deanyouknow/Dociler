//! Framed, versioned IPC protocol for the isolated document extraction worker.
//!
//! Stdout is reserved exclusively for the framed protocol. Diagnostics or
//! warnings must be directed to stderr.

use serde::{Deserialize, Serialize};

use crate::document::Document;

/// Framed header preceding the JSON payload on worker stdout.
pub const MAGIC_HEADER: &str = "DOCILER_EXTRACT_V1";

/// Current IPC protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Top-level result emitted by the worker process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerExtractionResult {
    pub version: u32,
    pub outcome: Result<Document, WorkerErrorPayload>,
}

impl WorkerExtractionResult {
    pub fn success(document: Document) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            outcome: Ok(document),
        }
    }

    pub fn failure(error: WorkerErrorPayload) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            outcome: Err(error),
        }
    }

    /// Serializes the result with the framing header and newline delimiter.
    pub fn to_framed_string(&self) -> Result<String, serde_json::Error> {
        let json = serde_json::to_string(self)?;
        Ok(format!("{MAGIC_HEADER}\n{json}\n"))
    }

    /// Parses a framed string from worker stdout.
    pub fn parse_framed(raw: &str) -> Result<Self, ProtocolError> {
        let trimmed = raw.trim_start();
        if !trimmed.starts_with(MAGIC_HEADER) {
            return Err(ProtocolError::MissingMagicHeader);
        }
        let after_header = trimmed[MAGIC_HEADER.len()..].trim_start();
        let json_line = after_header
            .lines()
            .next()
            .ok_or(ProtocolError::EmptyPayload)?;

        let result: Self =
            serde_json::from_str(json_line).map_err(|e| ProtocolError::Json(e.to_string()))?;
        if result.version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(result.version));
        }
        Ok(result)
    }
}

/// Detailed error payload returned by the extraction worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerErrorPayload {
    FileNotFound(String),
    PermissionDenied(String),
    EmptyFile,
    InvalidEncoding,
    EncryptedFile,
    ScannedPdfNoText,
    DecompressionLimitExceeded {
        limit_bytes: u64,
        observed_bytes: u64,
    },
    TextLimitExceeded {
        limit_bytes: u64,
        observed_bytes: u64,
    },
    MismatchedSignature {
        expected: String,
        found: String,
    },
    UnsupportedFormat(String),
    CorruptedDocument(String),
    Io(String),
}

/// Errors occurring during IPC protocol serialization or framing parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    MissingMagicHeader,
    EmptyPayload,
    UnsupportedVersion(u32),
    Json(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingMagicHeader => write!(f, "worker output missing {MAGIC_HEADER} header"),
            Self::EmptyPayload => write!(f, "worker output contained empty payload after header"),
            Self::UnsupportedVersion(v) => {
                write!(
                    f,
                    "unsupported worker protocol version {v}; expected {PROTOCOL_VERSION}"
                )
            }
            Self::Json(msg) => write!(f, "failed to parse worker JSON protocol: {msg}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentFormat, DocumentMetadata, DocumentSource};

    #[test]
    fn framed_protocol_roundtrip_success() {
        let doc = Document::new(
            DocumentSource::from_bytes("sample.txt", DocumentFormat::PlainText, b"hello"),
            DocumentMetadata::default(),
            Vec::new(),
        );
        let res = WorkerExtractionResult::success(doc.clone());
        let framed = res.to_framed_string().expect("serialize framed");
        assert!(framed.starts_with("DOCILER_EXTRACT_V1\n"));
        let parsed = WorkerExtractionResult::parse_framed(&framed).expect("parse framed");
        assert_eq!(parsed.version, PROTOCOL_VERSION);
        assert_eq!(parsed.outcome, Ok(doc));
    }

    #[test]
    fn framed_protocol_roundtrip_failure() {
        let err = WorkerErrorPayload::EncryptedFile;
        let res = WorkerExtractionResult::failure(err.clone());
        let framed = res.to_framed_string().expect("serialize framed");
        let parsed = WorkerExtractionResult::parse_framed(&framed).expect("parse framed");
        assert_eq!(parsed.outcome, Err(err));
    }

    #[test]
    fn framed_protocol_rejection() {
        assert!(WorkerExtractionResult::parse_framed("INVALID").is_err());
        assert!(WorkerExtractionResult::parse_framed("DOCILER_EXTRACT_V1\n").is_err());
        assert!(WorkerExtractionResult::parse_framed("DOCILER_EXTRACT_V1\nnot json").is_err());
    }
}
