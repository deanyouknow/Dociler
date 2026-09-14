//! OpenAI-compatible remote text backend with pinned DNS and no redirects.

use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::credentials::{CredentialId, Secret};
use crate::session::{Message, Role};

const MAX_PROFILE_FIELD: usize = 256;
const MAX_JSON_RESPONSE: u64 = 1024 * 1024;
const MAX_STREAM_RESPONSE: u64 = 8 * 1024 * 1024;
const MAX_SSE_LINE: usize = 256 * 1024;
const MAX_ANSWER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteProfile {
    name: String,
    base_url: String,
    model: String,
    #[serde(default)]
    credential: bool,
}

impl RemoteProfile {
    pub fn new(
        name: &str,
        base_url: &str,
        model: &str,
        credential: bool,
    ) -> Result<Self, RemoteError> {
        if !valid_id(name) || !valid_text(model) {
            return Err(RemoteError::InvalidProfile);
        }
        let base_url = normalize_base_url(base_url)?.to_string();
        Ok(Self {
            name: name.to_owned(),
            base_url,
            model: model.to_owned(),
            credential,
        })
    }

    pub fn validate(&self) -> Result<(), RemoteError> {
        let validated = Self::new(&self.name, &self.base_url, &self.model, self.credential)?;
        if validated != *self {
            return Err(RemoteError::InvalidProfile);
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn needs_credential(&self) -> bool {
        self.credential
    }

    pub fn credential_id(&self) -> CredentialId {
        // Profile validation guarantees this derived identifier is valid.
        CredentialId::new(&format!("remote-{}", self.name)).expect("validated profile name")
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_PROFILE_FIELD && !value.chars().any(char::is_control)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteError {
    InvalidProfile,
    UnsafeEndpoint,
    Resolution,
    Connection,
    Authentication,
    Upstream,
    InvalidResponse,
    ModelUnavailable,
    ResponseLimit,
    Cancelled,
    Output,
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidProfile => "invalid remote profile",
            Self::UnsafeEndpoint => "remote endpoint violates the transport policy",
            Self::Resolution => "remote endpoint could not be resolved safely",
            Self::Connection => "remote endpoint connection failed",
            Self::Authentication => "remote endpoint rejected authentication",
            Self::Upstream => "remote endpoint returned an error",
            Self::InvalidResponse => "remote endpoint returned an invalid response",
            Self::ModelUnavailable => "configured upstream model is unavailable",
            Self::ResponseLimit => "remote response exceeded the safety limit",
            Self::Cancelled => "remote response was cancelled",
            Self::Output => "response output failed",
        };
        f.write_str(message)
    }
}

impl std::error::Error for RemoteError {}

fn normalize_base_url(input: &str) -> Result<Url, RemoteError> {
    if input.len() > 2048 || input.chars().any(char::is_control) {
        return Err(RemoteError::UnsafeEndpoint);
    }
    let mut url = Url::parse(input).map_err(|_| RemoteError::UnsafeEndpoint)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RemoteError::UnsafeEndpoint);
    }
    match url.path().trim_end_matches('/') {
        "" => url.set_path("/v1/"),
        "/v1" => url.set_path("/v1/"),
        _ => return Err(RemoteError::UnsafeEndpoint),
    }
    Ok(url)
}

fn private_or_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

#[derive(Debug)]
struct ResolvedEndpoint {
    base_url: Url,
    host: String,
    addresses: Vec<SocketAddr>,
}

impl ResolvedEndpoint {
    fn resolve(profile: &RemoteProfile) -> Result<Self, RemoteError> {
        profile.validate()?;
        let base_url = normalize_base_url(profile.base_url())?;
        let host = base_url
            .host_str()
            .ok_or(RemoteError::UnsafeEndpoint)?
            .to_owned();
        let port = base_url
            .port_or_known_default()
            .ok_or(RemoteError::UnsafeEndpoint)?;
        let mut addresses: Vec<_> = match base_url.host() {
            Some(Host::Ipv4(ip)) => vec![SocketAddr::new(ip.into(), port)],
            Some(Host::Ipv6(ip)) => vec![SocketAddr::new(ip.into(), port)],
            Some(Host::Domain(domain)) => (domain, port)
                .to_socket_addrs()
                .map_err(|_| RemoteError::Resolution)?
                .collect(),
            None => return Err(RemoteError::UnsafeEndpoint),
        };
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.is_empty() {
            return Err(RemoteError::Resolution);
        }
        if base_url.scheme() == "http"
            && !addresses
                .iter()
                .all(|address| private_or_loopback(address.ip()))
        {
            return Err(RemoteError::UnsafeEndpoint);
        }
        Ok(Self {
            base_url,
            host,
            addresses,
        })
    }
}

pub struct RemoteClient<'a> {
    client: Client,
    profile: RemoteProfile,
    models_url: Url,
    chat_url: Url,
    secret: Option<&'a Secret>,
}

impl<'a> RemoteClient<'a> {
    pub fn connect(
        profile: RemoteProfile,
        secret: Option<&'a Secret>,
    ) -> Result<Self, RemoteError> {
        let endpoint = ResolvedEndpoint::resolve(&profile)?;
        let models_url = endpoint
            .base_url
            .join("models")
            .map_err(|_| RemoteError::UnsafeEndpoint)?;
        let chat_url = endpoint
            .base_url
            .join("chat/completions")
            .map_err(|_| RemoteError::UnsafeEndpoint)?;
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("dociler/", env!("CARGO_PKG_VERSION")))
            .resolve_to_addrs(&endpoint.host, &endpoint.addresses)
            .build()
            .map_err(|_| RemoteError::Connection)?;
        Ok(Self {
            client,
            profile,
            models_url,
            chat_url,
            secret,
        })
    }

    fn request(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(secret) = &self.secret {
            request.bearer_auth(secret.expose())
        } else {
            request
        }
    }

    pub fn verify(&self) -> Result<(), RemoteError> {
        let response = self
            .request(self.client.get(self.models_url.clone()))
            .send()
            .map_err(|_| RemoteError::Connection)?;
        let models: ModelsResponse = parse_json(response)?;
        if !models
            .data
            .iter()
            .any(|model| model.id == self.profile.model)
        {
            return Err(RemoteError::ModelUnavailable);
        }
        let body = serde_json::json!({
            "model": self.profile.model,
            "messages": [{"role":"user","content":"Reply with OK."}],
            "max_tokens": 4,
            "stream": false
        });
        let response = self
            .request(self.client.post(self.chat_url.clone()))
            .json(&body)
            .send()
            .map_err(|_| RemoteError::Connection)?;
        let completion: CompletionResponse = parse_json(response)?;
        if !completion
            .choices
            .iter()
            .any(|choice| !choice.message.content.trim().is_empty())
        {
            return Err(RemoteError::InvalidResponse);
        }
        Ok(())
    }

    pub fn stream_chat(
        &self,
        messages: &[Message],
        mut output: impl FnMut(&str) -> Result<(), RemoteError>,
    ) -> Result<String, RemoteError> {
        let messages: Vec<_> = messages
            .iter()
            .map(|message| RemoteMessage {
                role: match message.role() {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                content: message.text(),
            })
            .collect();
        let body = ChatRequest {
            model: self.profile.model(),
            messages: &messages,
            stream: true,
        };
        let response = self
            .request(self.client.post(self.chat_url.clone()))
            .json(&body)
            .send()
            .map_err(|_| RemoteError::Connection)?;
        check_status(&response)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            return Err(RemoteError::InvalidResponse);
        }
        parse_sse(response, &mut output)
    }
}

fn check_status(response: &Response) -> Result<(), RemoteError> {
    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(RemoteError::Authentication),
        _ => Err(RemoteError::Upstream),
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(mut response: Response) -> Result<T, RemoteError> {
    check_status(&response)?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_JSON_RESPONSE)
    {
        return Err(RemoteError::ResponseLimit);
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_JSON_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RemoteError::Connection)?;
    if bytes.len() as u64 > MAX_JSON_RESPONSE {
        return Err(RemoteError::ResponseLimit);
    }
    serde_json::from_slice(&bytes).map_err(|_| RemoteError::InvalidResponse)
}

fn parse_sse(
    response: Response,
    output: &mut impl FnMut(&str) -> Result<(), RemoteError>,
) -> Result<String, RemoteError> {
    let mut reader = BufReader::new(response.take(MAX_STREAM_RESPONSE + 1));
    let mut total = 0_u64;
    let mut line = Vec::new();
    let mut answer = String::new();
    loop {
        line.clear();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| RemoteError::Connection)?;
        if count == 0 {
            return Err(RemoteError::InvalidResponse);
        }
        total += count as u64;
        if total > MAX_STREAM_RESPONSE || line.len() > MAX_SSE_LINE {
            return Err(RemoteError::ResponseLimit);
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line == b"data: [DONE]" {
            return Ok(answer);
        }
        let Some(data) = line.strip_prefix(b"data:") else {
            continue;
        };
        let data = if data.first() == Some(&b' ') {
            &data[1..]
        } else {
            data
        };
        let chunk: StreamChunk =
            serde_json::from_slice(data).map_err(|_| RemoteError::InvalidResponse)?;
        for choice in chunk.choices {
            if let Some(content) = choice.delta.content {
                if content.len() > MAX_ANSWER_BYTES - answer.len() {
                    return Err(RemoteError::ResponseLimit);
                }
                output(&content)?;
                answer.push_str(&content);
            }
        }
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelItem>,
}
#[derive(Deserialize)]
struct ModelItem {
    id: String,
}
#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
}
#[derive(Deserialize)]
struct CompletionChoice {
    message: CompletionMessage,
}
#[derive(Deserialize)]
struct CompletionMessage {
    content: String,
}
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [RemoteMessage<'a>],
    stream: bool,
}
#[derive(Serialize)]
struct RemoteMessage<'a> {
    role: &'static str,
    content: &'a str,
}
#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}
#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}
#[derive(Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalization_is_strict() {
        assert_eq!(
            normalize_base_url("https://example.com").unwrap().as_str(),
            "https://example.com/v1/"
        );
        assert_eq!(
            normalize_base_url("http://127.0.0.1:8080/v1/")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8080/v1/"
        );
        for value in [
            "example.com",
            "ftp://example.com",
            "https://user:secret@example.com",
            "https://example.com/path",
            "https://example.com/v1?q=x",
            "https://example.com/v1#x",
            "https://example.com\n",
        ] {
            assert!(normalize_base_url(value).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn plaintext_requires_a_private_literal() {
        let public = RemoteProfile::new("public", "http://8.8.8.8/v1", "model", false).unwrap();
        assert_eq!(
            ResolvedEndpoint::resolve(&public).unwrap_err(),
            RemoteError::UnsafeEndpoint
        );
        for address in [
            "http://127.0.0.1:1",
            "http://10.0.0.1",
            "http://172.16.0.1",
            "http://192.168.1.1",
            "http://[::1]:1",
            "http://[fd00::1]",
        ] {
            let profile = RemoteProfile::new("private", address, "model", false).unwrap();
            assert!(ResolvedEndpoint::resolve(&profile).is_ok());
        }
    }
}
