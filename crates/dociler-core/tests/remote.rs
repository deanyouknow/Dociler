use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dociler_core::chat::{
    CancellationToken, ChatError, Generation, GenerationEvent, GenerationPoll, RemoteChatBackend,
};
use dociler_core::config::{ConfigSource, ConfigStore};
use dociler_core::credentials::Secret;
use dociler_core::paths::AppPaths;
use dociler_core::remote::{RemoteClient, RemoteError, RemoteProfile};
use dociler_core::session::{Role, Session};
use dociler_core::workspace::Workspace;

#[derive(Clone)]
struct Reply {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
    extra_headers: &'static str,
}

type CapturedRequests = Arc<Mutex<Vec<Vec<u8>>>>;
type MockServer = (String, CapturedRequests, thread::JoinHandle<()>);

fn json(body: &str) -> Reply {
    Reply {
        status: "200 OK",
        content_type: "application/json",
        body: body.as_bytes().to_vec(),
        extra_headers: "",
    }
}

fn sse(body: Vec<u8>) -> Reply {
    Reply {
        status: "200 OK",
        content_type: "text/event-stream; charset=utf-8",
        body,
        extra_headers: "",
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "connection ended before request headers");
        request.extend_from_slice(&buffer[..count]);
        assert!(request.len() <= 128 * 1024);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .map(str::to_owned)
        })
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while request.len() - header_end < length {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "connection ended before request body");
        request.extend_from_slice(&buffer[..count]);
    }
    request
}

fn serve(replies: Vec<Reply>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let thread_captured = Arc::clone(&captured);
    let handle = thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            thread_captured.lock().unwrap().push(request);
            write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
                reply.status,
                reply.content_type,
                reply.body.len(),
                reply.extra_headers
            )
            .unwrap();
            stream.write_all(&reply.body).unwrap();
            stream.flush().unwrap();
        }
    });
    (format!("http://{address}/v1"), captured, handle)
}

#[test]
fn verifies_then_streams_unicode_with_bearer_auth() {
    let chunks = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Halo \"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"dunia\"}}]}\r\n\r\n",
        "data: [DONE]\n\n"
    );
    let (url, captured, server) = serve(vec![
        json(r#"{"data":[{"id":"upstream-model"}]}"#),
        json(r#"{"choices":[{"message":{"content":"OK"}}]}"#),
        sse(chunks.as_bytes().to_vec()),
        sse(chunks.as_bytes().to_vec()),
    ]);
    let profile = RemoteProfile::new("test", &url, "upstream-model", true).unwrap();
    let secret = Secret::new("private-test-value".into());
    let client = RemoteClient::connect(profile, Some(&secret)).unwrap();
    client
        .verify_cancellable(&CancellationToken::new())
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(Workspace::open(dir.path()).unwrap());
    session.push(Role::User, "Jelaskan 日本語".into()).unwrap();
    let mut streamed = String::new();
    let answer = client
        .stream_chat(session.messages(), |text| {
            streamed.push_str(text);
            Ok(())
        })
        .unwrap();
    assert_eq!(streamed, "Halo dunia");
    assert_eq!(answer, streamed);
    let mut cancellable_streamed = String::new();
    let answer = client
        .stream_chat_cancellable(session.messages(), &CancellationToken::new(), |text| {
            cancellable_streamed.push_str(text);
            Ok(())
        })
        .unwrap();
    assert_eq!(cancellable_streamed, "Halo dunia");
    assert_eq!(answer, cancellable_streamed);
    server.join().unwrap();

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for request in requests.iter() {
        let text = String::from_utf8_lossy(request);
        assert!(text.contains("authorization: Bearer private-test-value\r\n"));
    }
    assert!(String::from_utf8_lossy(&requests[0]).starts_with("GET /v1/models HTTP/1.1"));
    assert!(
        String::from_utf8_lossy(&requests[1]).starts_with("POST /v1/chat/completions HTTP/1.1")
    );
    for request in &requests[2..] {
        let chat = String::from_utf8_lossy(request);
        assert!(chat.contains("Jelaskan 日本語"));
        assert!(chat.contains("\"stream\":true"));
    }
}

#[test]
fn rejects_redirects_missing_models_and_incomplete_streams() {
    let redirect = Reply {
        status: "302 Found",
        content_type: "text/plain",
        body: Vec::new(),
        extra_headers: "Location: http://127.0.0.1:1/v1/models\r\n",
    };
    let (url, _, server) = serve(vec![redirect]);
    let client =
        RemoteClient::connect(RemoteProfile::new("a", &url, "m", false).unwrap(), None).unwrap();
    assert_eq!(client.verify().unwrap_err(), RemoteError::Upstream);
    server.join().unwrap();

    let (url, _, server) = serve(vec![json(r#"{"data":[{"id":"other"}]}"#)]);
    let client =
        RemoteClient::connect(RemoteProfile::new("b", &url, "m", false).unwrap(), None).unwrap();
    assert_eq!(client.verify().unwrap_err(), RemoteError::ModelUnavailable);
    server.join().unwrap();

    let (url, _, server) = serve(vec![sse(b"data: {\"choices\":[]}\n\n".to_vec())]);
    let client =
        RemoteClient::connect(RemoteProfile::new("c", &url, "m", false).unwrap(), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(Workspace::open(dir.path()).unwrap());
    session.push(Role::User, "hello".into()).unwrap();
    assert_eq!(
        client
            .stream_chat(session.messages(), |_| Ok(()))
            .unwrap_err(),
        RemoteError::InvalidResponse
    );
    server.join().unwrap();
}

#[test]
fn rejects_oversized_sse_line_before_output() {
    let mut body = b"data: ".to_vec();
    body.extend(std::iter::repeat_n(b'x', 256 * 1024));
    body.push(b'\n');
    let (url, _, server) = serve(vec![sse(body)]);
    let client =
        RemoteClient::connect(RemoteProfile::new("limit", &url, "m", false).unwrap(), None)
            .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new(Workspace::open(dir.path()).unwrap());
    session.push(Role::User, "hello".into()).unwrap();
    let mut called = false;
    assert_eq!(
        client
            .stream_chat(session.messages(), |_| {
                called = true;
                Ok(())
            })
            .unwrap_err(),
        RemoteError::ResponseLimit
    );
    assert!(!called);
    server.join().unwrap();
}

#[test]
fn profile_configuration_roundtrips_without_a_secret() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(
        dir.path().join("config"),
        dir.path().join("data"),
        dir.path().join("cache"),
    )
    .unwrap();
    let store = ConfigStore::new(paths.clone());
    let mut settings = store.load().unwrap().settings;
    let profile = RemoteProfile::new("office", "https://example.com", "model-a", true).unwrap();
    settings.add_remote_profile(profile).unwrap();
    assert!(
        settings
            .add_remote_profile(
                RemoteProfile::new("office", "https://example.org", "model-b", false).unwrap()
            )
            .is_err()
    );
    store.save(&settings).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.source, ConfigSource::Saved);
    let profile = loaded.settings.remote_profile("office").unwrap();
    assert_eq!(profile.base_url(), "https://example.com/v1/");
    assert!(profile.needs_credential());
    let bytes = std::fs::read(paths.config_file()).unwrap();
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("private-test-value")
    );
    assert!(!paths.data_dir.exists());
    assert!(!paths.cache_dir.exists());
}

#[test]
fn cancellation_aborts_a_stalled_stream_before_transport_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
        ready_sender.send(()).unwrap();
        release_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
    });

    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(Workspace::open(dir.path()).unwrap());
    let profile =
        RemoteProfile::new("stall", &format!("http://{address}/v1"), "model", false).unwrap();
    let generation = Generation::start(
        session,
        "hello".to_owned(),
        Box::new(RemoteChatBackend::native(profile)),
    )
    .ok()
    .unwrap();
    ready_receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    let started = Instant::now();
    generation.cancel();
    let deadline = started + Duration::from_secs(1);
    loop {
        match generation.poll() {
            (
                GenerationPoll::Event,
                Some(GenerationEvent::Failed(_, ChatError::Remote(RemoteError::Cancelled))),
            ) => break,
            (GenerationPoll::Pending, None) => {
                assert!(Instant::now() < deadline, "cancel did not abort transport");
                thread::yield_now();
            }
            _ => panic!("unexpected generation state"),
        }
    }
    assert!(started.elapsed() < Duration::from_secs(1));
    release_sender.send(()).unwrap();
    server.join().unwrap();
}
