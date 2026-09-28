//! Consent-gated downloads of immutable assets from the built-in manifest.

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use reqwest::{StatusCode, Url};
use sha2::{Digest, Sha256};

use crate::assets::{
    AssetKind, AssetSpec, CacheState, VerificationLevel, inspect_cached_asset_cancellable,
};
use crate::cancellation::CancellationToken;
use crate::paths::AppPaths;

const READ_BUFFER_BYTES: usize = 1024 * 1024;
const CANCEL_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadOptions {
    pub confirmed: bool,
    pub restart_partial: bool,
}

impl DownloadOptions {
    pub const fn confirmed(restart_partial: bool) -> Self {
        Self {
            confirmed: true,
            restart_partial,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub resumed_from: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadOutcome {
    AlreadyVerified,
    Published {
        resumed_from: u64,
        partial_cleanup_warning: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadError {
    ConsentRequired,
    UnsupportedSource,
    UnsafePath,
    ExistingInvalid,
    InvalidPartial,
    Busy,
    Connection,
    Upstream,
    ResumeRejected,
    InvalidResponse,
    SizeMismatch,
    HashMismatch,
    Cancelled,
    PublishConflict,
    Io(io::ErrorKind),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ConsentRequired => "download consent is required",
            Self::UnsupportedSource => "asset source or redirect is not allowed",
            Self::UnsafePath => "asset cache path is unsafe",
            Self::ExistingInvalid => "an invalid final asset already exists",
            Self::InvalidPartial => "the partial asset is unsafe or invalid",
            Self::Busy => "another process is downloading this asset",
            Self::Connection => "asset connection failed",
            Self::Upstream => "asset server returned an error",
            Self::ResumeRejected => "asset server rejected the resume request",
            Self::InvalidResponse => "asset server returned inconsistent metadata",
            Self::SizeMismatch => "downloaded asset size does not match the manifest",
            Self::HashMismatch => "downloaded asset checksum does not match the manifest",
            Self::Cancelled => "asset download was cancelled",
            Self::PublishConflict => "the final asset changed while publishing",
            Self::Io(_) => "asset cache I/O failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for DownloadError {}

/// Download a manifest asset after explicit caller consent. The final path is
/// never overwritten; bytes remain in a `.partial` sibling until size and
/// SHA-256 verification succeed.
pub fn download_cached_asset(
    paths: &AppPaths,
    asset: AssetSpec,
    options: DownloadOptions,
    cancellation: &CancellationToken,
    progress: impl FnMut(DownloadProgress),
) -> Result<DownloadOutcome, DownloadError> {
    download_from_url(
        paths,
        asset,
        asset.source_url(),
        options,
        cancellation,
        progress,
        false,
    )
}

fn download_from_url(
    paths: &AppPaths,
    asset: AssetSpec,
    source: &str,
    options: DownloadOptions,
    cancellation: &CancellationToken,
    mut progress: impl FnMut(DownloadProgress),
    allow_test_loopback: bool,
) -> Result<DownloadOutcome, DownloadError> {
    if !options.confirmed {
        return Err(DownloadError::ConsentRequired);
    }
    if cancellation.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }

    let source = Url::parse(source).map_err(|_| DownloadError::UnsupportedSource)?;
    let source_family = SourceFamily::from_url(&source, allow_test_loopback)?;
    let final_path = asset.cache_path(paths);
    let parent = final_path.parent().ok_or(DownloadError::UnsafePath)?;
    create_private_asset_directory(paths, asset.kind(), parent)?;

    let lock_path = parent.join(".dociler-download.lock");
    let lock = open_managed_file(&lock_path, true, true)?;
    FileExt::try_lock_exclusive(&lock).map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            DownloadError::Busy
        } else {
            DownloadError::Io(error.kind())
        }
    })?;

    match inspect_cached_asset_cancellable(
        paths,
        asset,
        VerificationLevel::Sha256,
        Some(cancellation),
    )
    .state()
    {
        CacheState::Verified => return Ok(DownloadOutcome::AlreadyVerified),
        CacheState::Cancelled => return Err(DownloadError::Cancelled),
        CacheState::Missing => {}
        _ => return Err(DownloadError::ExistingInvalid),
    }

    let partial_path = partial_path(&final_path)?;
    if options.restart_partial {
        remove_managed_partial(&partial_path)?;
    }
    let (mut partial, resumed_from) = open_partial(&partial_path, asset.byte_size())?;
    let mut hasher = hash_existing(&mut partial, cancellation)?;
    progress(DownloadProgress {
        downloaded_bytes: resumed_from,
        total_bytes: asset.byte_size(),
        resumed_from,
    });

    if resumed_from < asset.byte_size() {
        let client = build_client(source_family.clone())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| DownloadError::Connection)?;
        runtime.block_on(receive(
            &client,
            source,
            Transfer {
                partial: &mut partial,
                hasher: &mut hasher,
                expected: asset.byte_size(),
                resumed_from,
                cancellation,
                progress: &mut progress,
            },
        ))?;
    }

    if cancellation.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let observed = partial
        .metadata()
        .map_err(|error| DownloadError::Io(error.kind()))?
        .len();
    if observed != asset.byte_size() {
        return Err(DownloadError::SizeMismatch);
    }
    if format!("{:x}", hasher.finalize()) != asset.sha256() {
        return Err(DownloadError::HashMismatch);
    }
    partial
        .sync_all()
        .map_err(|error| DownloadError::Io(error.kind()))?;
    drop(partial);

    match fs::hard_link(&partial_path, &final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return if inspect_cached_asset_cancellable(
                paths,
                asset,
                VerificationLevel::Sha256,
                Some(cancellation),
            )
            .state()
            .is_verified()
            {
                Ok(DownloadOutcome::AlreadyVerified)
            } else {
                Err(DownloadError::PublishConflict)
            };
        }
        Err(error) => return Err(DownloadError::Io(error.kind())),
    }
    sync_directory(parent)?;
    let partial_cleanup_warning = fs::remove_file(&partial_path).is_err();
    if !partial_cleanup_warning {
        sync_directory(parent)?;
    }
    Ok(DownloadOutcome::Published {
        resumed_from,
        partial_cleanup_warning,
    })
}

#[derive(Clone)]
enum SourceFamily {
    HuggingFace,
    GitHub,
    Loopback { host: String, port: Option<u16> },
}

impl SourceFamily {
    fn from_url(url: &Url, allow_test_loopback: bool) -> Result<Self, DownloadError> {
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(DownloadError::UnsupportedSource);
        }
        let host = url.host_str().ok_or(DownloadError::UnsupportedSource)?;
        if url.scheme() == "https" && domain_matches(host, "huggingface.co") {
            Ok(Self::HuggingFace)
        } else if url.scheme() == "https" && domain_matches(host, "github.com") {
            Ok(Self::GitHub)
        } else if allow_test_loopback
            && url.scheme() == "http"
            && matches!(host, "127.0.0.1" | "::1" | "localhost")
        {
            Ok(Self::Loopback {
                host: host.to_owned(),
                port: url.port_or_known_default(),
            })
        } else {
            Err(DownloadError::UnsupportedSource)
        }
    }

    fn allows(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return false;
        }
        match self {
            Self::HuggingFace => {
                url.scheme() == "https"
                    && (domain_matches(host, "huggingface.co") || domain_matches(host, "hf.co"))
            }
            Self::GitHub => {
                url.scheme() == "https"
                    && (domain_matches(host, "github.com")
                        || domain_matches(host, "githubusercontent.com"))
            }
            Self::Loopback {
                host: expected,
                port,
            } => {
                url.scheme() == "http"
                    && host.eq_ignore_ascii_case(expected)
                    && url.port_or_known_default() == *port
            }
        }
    }
}

fn domain_matches(host: &str, suffix: &str) -> bool {
    host.eq_ignore_ascii_case(suffix)
        || host
            .to_ascii_lowercase()
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn build_client(family: SourceFamily) -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("dociler/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 || !family.allows(attempt.url()) {
                attempt.error("disallowed asset redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| DownloadError::Connection)
}

struct Transfer<'a, P> {
    partial: &'a mut File,
    hasher: &'a mut Sha256,
    expected: u64,
    resumed_from: u64,
    cancellation: &'a CancellationToken,
    progress: &'a mut P,
}

async fn receive<P: FnMut(DownloadProgress)>(
    client: &reqwest::Client,
    source: Url,
    transfer: Transfer<'_, P>,
) -> Result<(), DownloadError> {
    let Transfer {
        partial,
        hasher,
        expected,
        resumed_from,
        cancellation,
        progress,
    } = transfer;
    let mut request = client.get(source).header(ACCEPT_ENCODING, "identity");
    if resumed_from > 0 {
        request = request.header(RANGE, format!("bytes={resumed_from}-"));
    }
    let response = await_response(request.send(), cancellation).await?;
    let status = response.status();
    if resumed_from > 0 && status != StatusCode::PARTIAL_CONTENT {
        return Err(DownloadError::ResumeRejected);
    }
    if resumed_from == 0 && status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
        return if status.is_client_error() || status.is_server_error() || status.is_redirection() {
            Err(DownloadError::Upstream)
        } else {
            Err(DownloadError::InvalidResponse)
        };
    }

    if status == StatusCode::PARTIAL_CONTENT {
        let value = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .ok_or(DownloadError::InvalidResponse)?;
        let (start, end, total) = parse_content_range(value)?;
        if start != resumed_from || total != expected || end < start || end >= total {
            return Err(DownloadError::InvalidResponse);
        }
        if let Some(length) = response.content_length() {
            if length != end - start + 1 {
                return Err(DownloadError::InvalidResponse);
            }
        }
    } else if let Some(length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        if length != expected {
            return Err(DownloadError::InvalidResponse);
        }
    }

    let mut downloaded = resumed_from;
    let mut stream = response.bytes_stream();
    loop {
        if cancellation.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let next = match tokio::time::timeout(CANCEL_POLL, stream.next()).await {
            Ok(next) => next,
            Err(_) => continue,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|_| DownloadError::Connection)?;
        if chunk.len() as u64 > expected.saturating_sub(downloaded) {
            return Err(DownloadError::SizeMismatch);
        }
        partial
            .write_all(&chunk)
            .map_err(|error| DownloadError::Io(error.kind()))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        progress(DownloadProgress {
            downloaded_bytes: downloaded,
            total_bytes: expected,
            resumed_from,
        });
    }
    if downloaded != expected {
        return Err(DownloadError::SizeMismatch);
    }
    Ok(())
}

async fn await_response<F>(
    future: F,
    cancellation: &CancellationToken,
) -> Result<reqwest::Response, DownloadError>
where
    F: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let mut future = Box::pin(future);
    loop {
        if cancellation.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        match tokio::time::timeout(CANCEL_POLL, &mut future).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(_)) => return Err(DownloadError::Connection),
            Err(_) => {}
        }
    }
}

fn parse_content_range(value: &str) -> Result<(u64, u64, u64), DownloadError> {
    let value = value
        .strip_prefix("bytes ")
        .ok_or(DownloadError::InvalidResponse)?;
    let (range, total) = value
        .split_once('/')
        .ok_or(DownloadError::InvalidResponse)?;
    let (start, end) = range
        .split_once('-')
        .ok_or(DownloadError::InvalidResponse)?;
    Ok((
        start.parse().map_err(|_| DownloadError::InvalidResponse)?,
        end.parse().map_err(|_| DownloadError::InvalidResponse)?,
        total.parse().map_err(|_| DownloadError::InvalidResponse)?,
    ))
}

fn create_private_asset_directory(
    paths: &AppPaths,
    kind: AssetKind,
    target: &Path,
) -> Result<(), DownloadError> {
    create_private_dir(&paths.data_dir)?;
    let cache_root = match kind {
        AssetKind::Model => paths.models_dir(),
        AssetKind::RuntimeArchive => paths.runtimes_dir(),
    };
    let relative_root = cache_root
        .strip_prefix(&paths.data_dir)
        .map_err(|_| DownloadError::UnsafePath)?;
    let relative_target = target
        .strip_prefix(&paths.data_dir)
        .map_err(|_| DownloadError::UnsafePath)?;
    if relative_root
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || relative_target
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || !target.starts_with(&cache_root)
    {
        return Err(DownloadError::UnsafePath);
    }
    let mut current = paths.data_dir.clone();
    for part in relative_target.components() {
        current.push(part);
        create_private_dir(&current)?;
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), DownloadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(DownloadError::UnsafePath);
            }
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(DownloadError::Io(error.kind())),
    }
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata =
                fs::symlink_metadata(path).map_err(|error| DownloadError::Io(error.kind()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                Err(DownloadError::UnsafePath)
            } else {
                Ok(())
            }
        }
        Err(error) => Err(DownloadError::Io(error.kind())),
    }
}

fn open_managed_file(path: &Path, create: bool, append: bool) -> Result<File, DownloadError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(DownloadError::UnsafePath);
        }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create).append(append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| DownloadError::Io(error.kind()))?;
    if !file
        .metadata()
        .map_err(|error| DownloadError::Io(error.kind()))?
        .is_file()
    {
        return Err(DownloadError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| DownloadError::Io(error.kind()))?;
    }
    Ok(file)
}

fn partial_path(final_path: &Path) -> Result<PathBuf, DownloadError> {
    let name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(DownloadError::UnsafePath)?;
    Ok(final_path.with_file_name(format!("{name}.partial")))
}

fn remove_managed_partial(path: &Path) -> Result<(), DownloadError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(DownloadError::Io(error.kind())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DownloadError::InvalidPartial);
    }
    fs::remove_file(path).map_err(|error| DownloadError::Io(error.kind()))
}

fn open_partial(path: &Path, expected: u64) -> Result<(File, u64), DownloadError> {
    let file = open_managed_file(path, true, true).map_err(|error| match error {
        DownloadError::UnsafePath => DownloadError::InvalidPartial,
        other => other,
    })?;
    let length = file
        .metadata()
        .map_err(|error| DownloadError::Io(error.kind()))?
        .len();
    if length > expected {
        return Err(DownloadError::InvalidPartial);
    }
    Ok((file, length))
}

fn hash_existing(
    file: &mut File,
    cancellation: &CancellationToken,
) -> Result<Sha256, DownloadError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    loop {
        if cancellation.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let count = file
            .read(&mut buffer)
            .map_err(|error| DownloadError::Io(error.kind()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher)
}

fn sync_directory(path: &Path) -> Result<(), DownloadError> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| DownloadError::Io(error.kind()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;

    const BODY: &[u8] = b"hello world";
    const BODY_HASH: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    fn paths(root: &Path) -> AppPaths {
        AppPaths::new(root.join("config"), root.join("data"), root.join("cache")).unwrap()
    }

    fn asset() -> AssetSpec {
        AssetSpec::test_fixture(AssetKind::Model, "fixture", "fixture.gguf", 11, BODY_HASH)
    }

    fn serve(
        status: &'static str,
        headers: Vec<(&'static str, String)>,
        body: &'static [u8],
        requests: Arc<Mutex<Vec<String>>>,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            requests.lock().unwrap().push(request);
            write!(stream, "HTTP/1.1 {status}\r\n").unwrap();
            for (name, value) in headers {
                write!(stream, "{name}: {value}\r\n").unwrap();
            }
            write!(stream, "Connection: close\r\n\r\n").unwrap();
            stream.write_all(body).unwrap();
        });
        (format!("http://{address}/asset"), handle)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut reader = BufReader::new(stream);
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            request.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        request
    }

    #[test]
    fn consent_is_required_before_filesystem_or_network_access() {
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let result = download_from_url(
            &app_paths,
            asset(),
            "http://127.0.0.1:1/never",
            DownloadOptions {
                confirmed: false,
                restart_partial: false,
            },
            &CancellationToken::new(),
            |_| {},
            true,
        );
        assert_eq!(result, Err(DownloadError::ConsentRequired));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn complete_download_is_verified_and_published_without_partial() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (url, server) = serve(
            "200 OK",
            vec![("Content-Length", BODY.len().to_string())],
            BODY,
            Arc::clone(&requests),
        );
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let outcome = download_from_url(
            &app_paths,
            asset(),
            &url,
            DownloadOptions::confirmed(false),
            &CancellationToken::new(),
            |_| {},
            true,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(
            outcome,
            DownloadOutcome::Published {
                resumed_from: 0,
                partial_cleanup_warning: false
            }
        );
        assert_eq!(fs::read(asset().cache_path(&app_paths)).unwrap(), BODY);
        assert!(
            !partial_path(&asset().cache_path(&app_paths))
                .unwrap()
                .exists()
        );
        assert!(
            asset()
                .cache_path(&app_paths)
                .parent()
                .unwrap()
                .join(".dociler-download.lock")
                .exists()
        );
        assert!(requests.lock().unwrap()[0].contains("accept-encoding: identity"));
    }

    #[test]
    fn partial_download_resumes_with_an_exact_range() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (url, server) = serve(
            "206 Partial Content",
            vec![
                ("Content-Length", "6".to_owned()),
                ("Content-Range", "bytes 5-10/11".to_owned()),
            ],
            &BODY[5..],
            Arc::clone(&requests),
        );
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let path = asset().cache_path(&app_paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(partial_path(&path).unwrap(), &BODY[..5]).unwrap();
        let outcome = download_from_url(
            &app_paths,
            asset(),
            &url,
            DownloadOptions::confirmed(false),
            &CancellationToken::new(),
            |_| {},
            true,
        )
        .unwrap();
        server.join().unwrap();
        assert!(matches!(
            outcome,
            DownloadOutcome::Published {
                resumed_from: 5,
                ..
            }
        ));
        assert!(requests.lock().unwrap()[0].contains("range: bytes=5-"));
        assert_eq!(fs::read(path).unwrap(), BODY);
    }

    #[test]
    fn ignored_resume_does_not_append_or_destroy_partial() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (url, server) = serve(
            "200 OK",
            vec![("Content-Length", BODY.len().to_string())],
            BODY,
            requests,
        );
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let path = asset().cache_path(&app_paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let partial = partial_path(&path).unwrap();
        fs::write(&partial, &BODY[..5]).unwrap();
        let result = download_from_url(
            &app_paths,
            asset(),
            &url,
            DownloadOptions::confirmed(false),
            &CancellationToken::new(),
            |_| {},
            true,
        );
        server.join().unwrap();
        assert_eq!(result, Err(DownloadError::ResumeRejected));
        assert_eq!(fs::read(partial).unwrap(), &BODY[..5]);
        assert!(!path.exists());
    }

    #[test]
    fn checksum_failure_keeps_partial_and_never_publishes() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let wrong = b"hello worle";
        let (url, server) = serve(
            "200 OK",
            vec![("Content-Length", wrong.len().to_string())],
            wrong,
            requests,
        );
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let path = asset().cache_path(&app_paths);
        let result = download_from_url(
            &app_paths,
            asset(),
            &url,
            DownloadOptions::confirmed(false),
            &CancellationToken::new(),
            |_| {},
            true,
        );
        server.join().unwrap();
        assert_eq!(result, Err(DownloadError::HashMismatch));
        assert_eq!(fs::read(partial_path(&path).unwrap()).unwrap(), wrong);
        assert!(!path.exists());
    }

    #[test]
    fn explicit_restart_replaces_only_the_managed_partial() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (url, server) = serve(
            "200 OK",
            vec![("Content-Length", BODY.len().to_string())],
            BODY,
            Arc::clone(&requests),
        );
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let path = asset().cache_path(&app_paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(partial_path(&path).unwrap(), b"hello worle").unwrap();
        let outcome = download_from_url(
            &app_paths,
            asset(),
            &url,
            DownloadOptions::confirmed(true),
            &CancellationToken::new(),
            |_| {},
            true,
        )
        .unwrap();
        server.join().unwrap();
        assert!(matches!(
            outcome,
            DownloadOutcome::Published {
                resumed_from: 0,
                ..
            }
        ));
        assert!(!requests.lock().unwrap()[0].contains("range:"));
        assert_eq!(fs::read(path).unwrap(), BODY);
    }

    #[test]
    fn concurrent_writer_lock_fails_without_connecting() {
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let path = asset().cache_path(&app_paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock = open_managed_file(
            &path.parent().unwrap().join(".dociler-download.lock"),
            true,
            true,
        )
        .unwrap();
        FileExt::try_lock_exclusive(&lock).unwrap();
        let result = download_from_url(
            &app_paths,
            asset(),
            "http://127.0.0.1:1/never",
            DownloadOptions::confirmed(false),
            &CancellationToken::new(),
            |_| {},
            true,
        );
        assert_eq!(result, Err(DownloadError::Busy));
        assert!(!path.exists());
    }

    #[test]
    fn verified_final_short_circuits_without_network() {
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let path = asset().cache_path(&app_paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, BODY).unwrap();
        let result = download_from_url(
            &app_paths,
            asset(),
            "http://127.0.0.1:1/never",
            DownloadOptions::confirmed(false),
            &CancellationToken::new(),
            |_| {},
            true,
        );
        assert_eq!(result, Ok(DownloadOutcome::AlreadyVerified));
        assert_eq!(fs::read(path).unwrap(), BODY);
    }

    #[test]
    fn cancellation_is_checked_before_connecting_and_keeps_partial() {
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let path = asset().cache_path(&app_paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let partial = partial_path(&path).unwrap();
        fs::write(&partial, &BODY[..5]).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = download_from_url(
            &app_paths,
            asset(),
            "http://127.0.0.1:1/never",
            DownloadOptions::confirmed(false),
            &cancellation,
            |_| {},
            true,
        );
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert_eq!(fs::read(partial).unwrap(), &BODY[..5]);
    }

    #[test]
    fn cancellation_aborts_a_stalled_body_and_keeps_partial() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.flush().unwrap();
            let _ = release_receiver.recv_timeout(Duration::from_secs(2));
        });
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        let cancellation = CancellationToken::new();
        let canceller = cancellation.clone();
        let cancellation_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            canceller.cancel();
        });
        let started = Instant::now();
        let result = download_from_url(
            &app_paths,
            asset(),
            &format!("http://{address}/asset"),
            DownloadOptions::confirmed(false),
            &cancellation,
            |_| {},
            true,
        );
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(1));
        let path = asset().cache_path(&app_paths);
        assert_eq!(fs::metadata(partial_path(&path).unwrap()).unwrap().len(), 0);
        assert!(!path.exists());
        cancellation_thread.join().unwrap();
        release_sender.send(()).unwrap();
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn managed_files_and_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let requests = Arc::new(Mutex::new(Vec::new()));
        let (url, server) = serve(
            "200 OK",
            vec![("Content-Length", BODY.len().to_string())],
            BODY,
            requests,
        );
        let directory = tempfile::tempdir().unwrap();
        let app_paths = paths(directory.path());
        download_from_url(
            &app_paths,
            asset(),
            &url,
            DownloadOptions::confirmed(false),
            &CancellationToken::new(),
            |_| {},
            true,
        )
        .unwrap();
        server.join().unwrap();
        let path = asset().cache_path(&app_paths);
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
