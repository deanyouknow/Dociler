//! Sandboxed document extraction boundary and binary format parsers.
//!
//! Provides out-of-process isolation for word-processing formats (.docx, .pdf,
//! .rtf, .odt, .doc) with strict memory ceilings, decompression limits, and
//! cancellation guards, communicating via a framed, versioned IPC protocol.

pub mod doc;
pub mod docx;
pub mod odt;
pub mod pdf;
pub mod protocol;
pub mod rtf;
pub mod xml;

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::cancellation::CancellationToken;
use crate::document::{
    Document, DocumentFormat, DocumentFormatError, parse_markdown, parse_plain_text,
};
use crate::extractor::protocol::{WorkerErrorPayload, WorkerExtractionResult};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_MAX_INPUT_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB
const DEFAULT_MAX_DECOMPRESSED_BYTES: u64 = 100 * 1024 * 1024; // 100 MiB
const DEFAULT_MAX_TEXT_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
const DEFAULT_MAX_RESIDENT_MEMORY_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB
const DIAGNOSTIC_LIMIT: usize = 8 * 1024; // 8 KiB
const POLL_INTERVAL: Duration = Duration::from_millis(15);

/// Operational resource and safety limits enforced on extraction.
#[derive(Debug, Clone)]
pub struct ExtractionLimits {
    pub timeout: Duration,
    pub max_input_bytes: u64,
    pub max_decompressed_bytes: u64,
    pub max_text_bytes: u64,
    pub max_resident_memory_bytes: Option<u64>,
    pub max_metadata_count: usize,
    pub worker_executable: Option<PathBuf>,
}

impl Default for ExtractionLimits {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_decompressed_bytes: DEFAULT_MAX_DECOMPRESSED_BYTES,
            max_text_bytes: DEFAULT_MAX_TEXT_BYTES,
            max_resident_memory_bytes: Some(DEFAULT_MAX_RESIDENT_MEMORY_BYTES),
            max_metadata_count: 100,
            worker_executable: None,
        }
    }
}

/// Errors occurring during document extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractorError {
    FileNotFound(PathBuf),
    PermissionDenied(PathBuf),
    InputTooLarge {
        limit_bytes: u64,
        actual_bytes: u64,
    },
    EmptyFile,
    Format(DocumentFormatError),
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
    MemoryLimitExceeded {
        limit_bytes: u64,
        observed_bytes: u64,
    },
    Timeout(Duration),
    Cancelled,
    WorkerSpawnFailed(String),
    WorkerExitedUnexpectedly {
        code: Option<i32>,
        stderr: String,
    },
    Protocol(String),
    CorruptedDocument(String),
    UnsupportedFormat(String),
    Io(String),
}

impl fmt::Display for ExtractorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileNotFound(path) => write!(f, "document file not found: {}", path.display()),
            Self::PermissionDenied(path) => {
                write!(f, "permission denied reading document: {}", path.display())
            }
            Self::InputTooLarge {
                limit_bytes,
                actual_bytes,
            } => {
                write!(
                    f,
                    "document size ({actual_bytes} bytes) exceeds limit ({limit_bytes} bytes)"
                )
            }
            Self::EmptyFile => write!(f, "document is empty (0 bytes)"),
            Self::Format(err) => write!(f, "document format error: {err}"),
            Self::EncryptedFile => write!(
                f,
                "document is password-protected or encrypted; encrypted files are rejected"
            ),
            Self::ScannedPdfNoText => {
                write!(
                    f,
                    "No embedded text found in PDF; OCR is not included in v1"
                )
            }
            Self::DecompressionLimitExceeded {
                limit_bytes,
                observed_bytes,
            } => {
                write!(
                    f,
                    "document decompression ({observed_bytes} bytes) exceeded limit ({limit_bytes} bytes)"
                )
            }
            Self::TextLimitExceeded {
                limit_bytes,
                observed_bytes,
            } => {
                write!(
                    f,
                    "extracted text ({observed_bytes} bytes) exceeded limit ({limit_bytes} bytes)"
                )
            }
            Self::MemoryLimitExceeded {
                limit_bytes,
                observed_bytes,
            } => {
                write!(
                    f,
                    "extraction worker memory ({observed_bytes} bytes) exceeded ceiling ({limit_bytes} bytes)"
                )
            }
            Self::Timeout(dur) => write!(f, "document extraction timed out after {dur:?}"),
            Self::Cancelled => write!(f, "document extraction was cancelled"),
            Self::WorkerSpawnFailed(msg) => write!(f, "failed to spawn extraction worker: {msg}"),
            Self::WorkerExitedUnexpectedly { code, stderr } => {
                write!(
                    f,
                    "extraction worker exited unexpectedly (code {code:?}): {stderr}"
                )
            }
            Self::Protocol(msg) => write!(f, "worker extraction protocol error: {msg}"),
            Self::CorruptedDocument(msg) => write!(f, "corrupted document structure: {msg}"),
            Self::UnsupportedFormat(ext) => write!(f, "unsupported document format: .{ext}"),
            Self::Io(msg) => write!(f, "document extraction I/O error: {msg}"),
        }
    }
}

impl std::error::Error for ExtractorError {}

/// In-process parsing dispatcher for all supported formats.
pub fn extract_document_in_process(
    _path: &Path,
    bytes: &[u8],
    format: DocumentFormat,
    display_name: &str,
    limits: &ExtractionLimits,
) -> Result<Document, ExtractorError> {
    if bytes.is_empty() {
        return Err(ExtractorError::EmptyFile);
    }
    if bytes.len() as u64 > limits.max_input_bytes {
        return Err(ExtractorError::InputTooLarge {
            limit_bytes: limits.max_input_bytes,
            actual_bytes: bytes.len() as u64,
        });
    }

    match format {
        DocumentFormat::PlainText => {
            parse_plain_text(display_name, bytes).map_err(ExtractorError::Format)
        }
        DocumentFormat::Markdown => {
            parse_markdown(display_name, bytes).map_err(ExtractorError::Format)
        }
        DocumentFormat::Docx => docx::extract_docx(bytes, display_name, limits),
        DocumentFormat::Odt => odt::extract_odt(bytes, display_name, limits),
        DocumentFormat::Rtf => rtf::extract_rtf(bytes, display_name, limits),
        DocumentFormat::Pdf => pdf::extract_pdf(bytes, display_name, limits),
        DocumentFormat::Doc => doc::extract_doc(bytes, display_name, limits),
    }
}

/// Extracts a document through an out-of-process sandboxed worker.
pub fn extract_document(
    path: &Path,
    limits: &ExtractionLimits,
    cancellation: Option<&CancellationToken>,
) -> Result<Document, ExtractorError> {
    extract_document_sandboxed(path, limits, cancellation)
}

/// Executes document extraction in an isolated child worker process.
pub fn extract_document_sandboxed(
    path: &Path,
    limits: &ExtractionLimits,
    cancellation: Option<&CancellationToken>,
) -> Result<Document, ExtractorError> {
    if !path.exists() {
        return Err(ExtractorError::FileNotFound(path.to_path_buf()));
    }
    if path.is_dir() {
        return Err(ExtractorError::Io(format!(
            "path is a directory: {}",
            path.display()
        )));
    }

    let meta = fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            ExtractorError::PermissionDenied(path.to_path_buf())
        }
        std::io::ErrorKind::NotFound => ExtractorError::FileNotFound(path.to_path_buf()),
        _ => ExtractorError::Io(e.to_string()),
    })?;

    if meta.len() == 0 {
        return Err(ExtractorError::EmptyFile);
    }
    if meta.len() > limits.max_input_bytes {
        return Err(ExtractorError::InputTooLarge {
            limit_bytes: limits.max_input_bytes,
            actual_bytes: meta.len(),
        });
    }

    let canonical_path = path.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            ExtractorError::PermissionDenied(path.to_path_buf())
        }
        _ => ExtractorError::Io(e.to_string()),
    })?;

    let worker_bin = if let Some(bin) = &limits.worker_executable {
        bin.clone()
    } else if let Ok(bin_str) = std::env::var("DOCILER_BIN") {
        PathBuf::from(bin_str)
    } else {
        std::env::current_exe().map_err(|e| ExtractorError::WorkerSpawnFailed(e.to_string()))?
    };

    let mut command = Command::new(&worker_bin);
    command
        .arg("__worker-extract")
        .arg(&canonical_path)
        .arg("--max-decompressed-bytes")
        .arg(limits.max_decompressed_bytes.to_string())
        .arg("--max-text-bytes")
        .arg(limits.max_text_bytes.to_string())
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        command.env("PATH", "/usr/bin:/bin");
    }

    #[cfg(target_os = "linux")]
    if let Some(parent) = worker_bin.parent() {
        command.env("LD_LIBRARY_PATH", parent);
    }

    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }

    let mut child = command
        .spawn()
        .map_err(|e| ExtractorError::WorkerSpawnFailed(e.to_string()))?;

    let child_pid = child.id();
    let deadline = Instant::now() + limits.timeout;

    // Monitor child process lifecycle
    let status = loop {
        if let Some(c) = cancellation {
            if c.is_cancelled() {
                let _ = kill_child_process_group(&mut child);
                return Err(ExtractorError::Cancelled);
            }
        }

        if Instant::now() >= deadline {
            let _ = kill_child_process_group(&mut child);
            return Err(ExtractorError::Timeout(limits.timeout));
        }

        if let Some(ceiling) = limits.max_resident_memory_bytes {
            if let Some(rss) = sample_child_rss(child_pid) {
                if rss > ceiling {
                    let _ = kill_child_process_group(&mut child);
                    return Err(ExtractorError::MemoryLimitExceeded {
                        limit_bytes: ceiling,
                        observed_bytes: rss,
                    });
                }
            }
        }

        match child.try_wait() {
            Ok(Some(exit_status)) => break exit_status,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(e) => {
                let _ = kill_child_process_group(&mut child);
                return Err(ExtractorError::Io(e.to_string()));
            }
        }
    };

    // Collect bounded stdout and stderr
    let mut stdout_bytes = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let max_read = (limits.max_text_bytes.saturating_mul(2)).max(65536) as usize;
        let mut take = (&mut stdout).take(max_read as u64);
        let _ = take.read_to_end(&mut stdout_bytes);
    }

    let mut stderr_bytes = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let mut take = (&mut stderr).take(DIAGNOSTIC_LIMIT as u64);
        let _ = take.read_to_end(&mut stderr_bytes);
    }
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

    let stdout_str = String::from_utf8_lossy(&stdout_bytes);
    let parsed_result = WorkerExtractionResult::parse_framed(&stdout_str);

    match parsed_result {
        Ok(result) => match result.outcome {
            Ok(doc) => Ok(doc),
            Err(payload) => Err(map_payload_to_error(payload)),
        },
        Err(proto_err) => {
            if !status.success() {
                Err(ExtractorError::WorkerExitedUnexpectedly {
                    code: status.code(),
                    stderr: stderr_str,
                })
            } else {
                Err(ExtractorError::Protocol(proto_err.to_string()))
            }
        }
    }
}

fn map_payload_to_error(payload: WorkerErrorPayload) -> ExtractorError {
    match payload {
        WorkerErrorPayload::FileNotFound(p) => ExtractorError::FileNotFound(PathBuf::from(p)),
        WorkerErrorPayload::PermissionDenied(p) => {
            ExtractorError::PermissionDenied(PathBuf::from(p))
        }
        WorkerErrorPayload::EmptyFile => ExtractorError::EmptyFile,
        WorkerErrorPayload::InvalidEncoding => {
            ExtractorError::Format(DocumentFormatError::InvalidEncoding)
        }
        WorkerErrorPayload::EncryptedFile => ExtractorError::EncryptedFile,
        WorkerErrorPayload::ScannedPdfNoText => ExtractorError::ScannedPdfNoText,
        WorkerErrorPayload::DecompressionLimitExceeded {
            limit_bytes,
            observed_bytes,
        } => ExtractorError::DecompressionLimitExceeded {
            limit_bytes,
            observed_bytes,
        },
        WorkerErrorPayload::TextLimitExceeded {
            limit_bytes,
            observed_bytes,
        } => ExtractorError::TextLimitExceeded {
            limit_bytes,
            observed_bytes,
        },
        WorkerErrorPayload::MismatchedSignature { expected: _, found } => {
            ExtractorError::CorruptedDocument(format!("mismatched signature: {found}"))
        }
        WorkerErrorPayload::UnsupportedFormat(ext) => ExtractorError::UnsupportedFormat(ext),
        WorkerErrorPayload::CorruptedDocument(msg) => ExtractorError::CorruptedDocument(msg),
        WorkerErrorPayload::Io(msg) => ExtractorError::Io(msg),
    }
}

/// Terminates child and waits for exit.
fn kill_child_process_group(child: &mut std::process::Child) -> Result<(), std::io::Error> {
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Samples child resident memory in bytes on Linux.
#[cfg(target_os = "linux")]
fn sample_child_rss(pid: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if let Some(val_str) = parts.first() {
                if let Ok(kb) = val_str.parse::<u64>() {
                    return Some(kb.saturating_mul(1024));
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn sample_child_rss(_pid: u32) -> Option<u64> {
    None
}

/// CLI entry point when invoked as `dociler __worker-extract <PATH> [OPTIONS]`.
pub fn run_worker_extract_cli(args: &[OsString]) -> i32 {
    let mut path_opt: Option<PathBuf> = None;
    let mut limits = ExtractionLimits::default();
    let mut display_name_opt: Option<String> = None;

    let mut idx = 0;
    while idx < args.len() {
        let arg = args[idx].to_string_lossy();
        if arg == "--max-decompressed-bytes" {
            idx += 1;
            if idx < args.len() {
                if let Ok(val) = args[idx].to_string_lossy().parse::<u64>() {
                    limits.max_decompressed_bytes = val;
                }
            }
        } else if arg == "--max-text-bytes" {
            idx += 1;
            if idx < args.len() {
                if let Ok(val) = args[idx].to_string_lossy().parse::<u64>() {
                    limits.max_text_bytes = val;
                }
            }
        } else if arg == "--display-name" {
            idx += 1;
            if idx < args.len() {
                display_name_opt = Some(args[idx].to_string_lossy().into_owned());
            }
        } else if !arg.starts_with("--") && path_opt.is_none() {
            path_opt = Some(PathBuf::from(arg.into_owned()));
        }
        idx += 1;
    }

    let Some(path) = path_opt else {
        let res = WorkerExtractionResult::failure(WorkerErrorPayload::Io(
            "missing document path argument".to_string(),
        ));
        emit_worker_result(&res);
        return 0;
    };

    let display_name = display_name_opt.unwrap_or_else(|| {
        path.file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    });

    let format = match DocumentFormat::from_path(&path) {
        Some(fmt) => fmt,
        None => {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unknown".to_string());
            let res = WorkerExtractionResult::failure(WorkerErrorPayload::UnsupportedFormat(ext));
            emit_worker_result(&res);
            return 0;
        }
    };

    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            let payload = match e.kind() {
                std::io::ErrorKind::NotFound => {
                    WorkerErrorPayload::FileNotFound(path.display().to_string())
                }
                std::io::ErrorKind::PermissionDenied => {
                    WorkerErrorPayload::PermissionDenied(path.display().to_string())
                }
                _ => WorkerErrorPayload::Io(e.to_string()),
            };
            let res = WorkerExtractionResult::failure(payload);
            emit_worker_result(&res);
            return 0;
        }
    };

    let outcome = extract_document_in_process(&path, &bytes, format, &display_name, &limits);
    let worker_res = match outcome {
        Ok(doc) => WorkerExtractionResult::success(doc),
        Err(err) => WorkerExtractionResult::failure(match err {
            ExtractorError::FileNotFound(p) => {
                WorkerErrorPayload::FileNotFound(p.display().to_string())
            }
            ExtractorError::PermissionDenied(p) => {
                WorkerErrorPayload::PermissionDenied(p.display().to_string())
            }
            ExtractorError::EmptyFile => WorkerErrorPayload::EmptyFile,
            ExtractorError::Format(DocumentFormatError::InvalidEncoding) => {
                WorkerErrorPayload::InvalidEncoding
            }
            ExtractorError::EncryptedFile => WorkerErrorPayload::EncryptedFile,
            ExtractorError::ScannedPdfNoText => WorkerErrorPayload::ScannedPdfNoText,
            ExtractorError::DecompressionLimitExceeded {
                limit_bytes,
                observed_bytes,
            } => WorkerErrorPayload::DecompressionLimitExceeded {
                limit_bytes,
                observed_bytes,
            },
            ExtractorError::TextLimitExceeded {
                limit_bytes,
                observed_bytes,
            } => WorkerErrorPayload::TextLimitExceeded {
                limit_bytes,
                observed_bytes,
            },
            ExtractorError::CorruptedDocument(msg) => WorkerErrorPayload::CorruptedDocument(msg),
            ExtractorError::UnsupportedFormat(ext) => WorkerErrorPayload::UnsupportedFormat(ext),
            other => WorkerErrorPayload::Io(other.to_string()),
        }),
    };

    emit_worker_result(&worker_res);
    0
}

fn emit_worker_result(res: &WorkerExtractionResult) {
    if let Ok(framed) = res.to_framed_string() {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(framed.as_bytes());
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn extract_plain_text_in_process() {
        let text = b"First paragraph.\n\nSecond paragraph.";
        let limits = ExtractionLimits::default();
        let doc = extract_document_in_process(
            Path::new("doc.txt"),
            text,
            DocumentFormat::PlainText,
            "doc.txt",
            &limits,
        )
        .expect("extract text");

        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.source.format, DocumentFormat::PlainText);
    }

    #[test]
    fn extract_markdown_in_process() {
        let md = b"# Heading 1\n\nSome **bold** text.";
        let limits = ExtractionLimits::default();
        let doc = extract_document_in_process(
            Path::new("doc.md"),
            md,
            DocumentFormat::Markdown,
            "doc.md",
            &limits,
        )
        .expect("extract markdown");

        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.source.format, DocumentFormat::Markdown);
    }

    #[test]
    fn text_limit_enforcement() {
        let limits = ExtractionLimits {
            max_text_bytes: 10,
            ..Default::default()
        };
        let rtf_data = br#"{\rtf1\ansi\deff0 This is some text that exceeds the small limit.\par}"#;
        let err = extract_document_in_process(
            Path::new("doc.rtf"),
            rtf_data,
            DocumentFormat::Rtf,
            "doc.rtf",
            &limits,
        )
        .unwrap_err();

        match err {
            ExtractorError::TextLimitExceeded { limit_bytes, .. } => {
                assert_eq!(limit_bytes, 10);
            }
            other => panic!("expected TextLimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn empty_file_rejection() {
        let limits = ExtractionLimits::default();
        let err = extract_document_in_process(
            Path::new("empty.txt"),
            b"",
            DocumentFormat::PlainText,
            "empty.txt",
            &limits,
        )
        .unwrap_err();

        assert_eq!(err, ExtractorError::EmptyFile);
    }

    #[test]
    fn pre_cancelled_worker_aborts() {
        let file = NamedTempFile::new().expect("create temp file");
        fs::write(file.path(), b"Hello world").unwrap();

        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let limits = ExtractionLimits::default();
        let err =
            extract_document_sandboxed(file.path(), &limits, Some(&cancellation)).unwrap_err();
        assert_eq!(err, ExtractorError::Cancelled);
    }

    #[test]
    fn worker_cli_missing_arg() {
        let code = run_worker_extract_cli(&[]);
        assert_eq!(code, 0);
    }

    #[test]
    fn worker_cli_with_file() {
        let file = NamedTempFile::new().expect("temp file");
        let path = file.path().with_extension("txt");
        fs::write(&path, b"Sample CLI worker input").unwrap();

        let code = run_worker_extract_cli(&[path.as_os_str().to_os_string()]);
        assert_eq!(code, 0);
        let _ = fs::remove_file(path);
    }
}
