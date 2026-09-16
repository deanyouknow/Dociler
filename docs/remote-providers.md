# Remote Providers and Text Chat

## Current capability

M3 can validate, save, list, and call an OpenAI-compatible remote profile. It
supports text messages only and streams OpenAI-style SSE responses through both
a scriptable one-shot command and an initial interactive, multi-turn TUI.

It does not read or upload workspace files, inject document skills, persist a
transcript, follow redirects, start Dociler's LAN API, or use a local model.
Document transmission and its destination-bound consent flow remain disabled
until the document pipeline exists.

## Commands

Verify without saving:

```sh
dociler connect verify https://models.example.com/v1 upstream-model
```

Save a verified profile:

```sh
dociler connect add office https://models.example.com/v1 upstream-model
dociler connect list
```

Rotate or remove credentials and profiles:

```sh
DOCILER_API_KEY='replacement' dociler connect key office
dociler connect key-clear office --confirm
dociler connect remove office --confirm
```

For an authenticated endpoint, provide `DOCILER_API_KEY` in the process
environment while running `connect verify` or `connect add`. The key is never a
command argument and is never printed. `connect key` also requires a non-empty
`DOCILER_API_KEY`; `connect key-clear` verifies keyless access before removing
the native key and therefore requires `--confirm`. `verify` uses a key
transiently and saves nothing. `add` stores it in macOS Keychain, Windows Credential Manager, or Linux
Secret Service, then writes only a `credential: true` marker to settings. Clear
the environment variable after use. If the OS credential service is locked or
unavailable, `add` fails—there is no plaintext-file fallback.

For the current one-shot chat, run the command and enter a prompt followed by
EOF (Ctrl-D on macOS/Linux or Ctrl-Z then Enter on Windows):

```sh
dociler run office
```

The prompt must be non-empty UTF-8 and at most 64 KiB. Dociler holds it and the
answer only in the process session, streams answer fragments to stdout, then
exits. Redirect stdout deliberately if the user wants to save an answer; Dociler
does not create a transcript or document itself.

For interactive chat, use either of these from a real terminal:

```sh
dociler
dociler chat office
```

If a profile name is omitted, Dociler selects the first saved profile. Use
`/connect` to list profiles, `/connect NAME` to switch, or `/connect add` for the
four-step profile wizard. `/connect remove NAME` requires repeating the exact
command, and `/connect key NAME` opens a masked field where an empty value means
verified keyless access. Switching, removing the active profile, or updating its
credential resets model context. `/status`, `/help`,
the two-step `/clear`, and `/exit` are also active.
History and rendered transcript remain only in process memory. A redirected
bare invocation prints help, and `chat` refuses to start without terminal input
and output, so scripts do not receive terminal control sequences.

The wizard collects a profile name, endpoint URL, exact model ID, and optional
API key. Key input is masked, held in zeroizing memory, and discarded after the
attempt. Verification and streamed chat use a cancellable async HTTP path, so
Escape/Ctrl+C drops even a stalled request rather than waiting for the 120-second
request timeout. Cancellation after response fragments leaves partial output
visible but never adds it to subsequent model context. OS DNS lookup and native
credential-store calls are still blocking platform operations.

Use `/back` to revisit setup fields after validation or verification fails;
hidden key input is always discarded after an attempt. Duplicate names fail
without replacement. Removal writes the non-secret configuration first and then
deletes any native credential, reporting an actionable warning if cleanup fails.
Credential rotation verifies the candidate policy before mutation and rolls a
new key back if the configuration marker cannot be saved. Upstream endpoint/model
editing, automatic reconnect state, and refreshed concurrent-process state are
still pending.

## Endpoint policy

- Include an explicit `http://` or `https://` scheme. A root endpoint is
  normalized to `/v1/`; an existing `/v1` or `/v1/` is accepted. Other paths,
  URL credentials, queries, and fragments are rejected.
- Plaintext HTTP is allowed only when every resolved address is IPv4 private or
  loopback, or IPv6 unique-local or loopback. Public endpoints require HTTPS.
- DNS is resolved once per client construction and all returned socket addresses
  are pinned into that client's connector. A later operation creates a new
  client and validates again.
- Ambient HTTP proxy variables are ignored for provider calls so a local/private
  request is not silently sent through a proxy.
- Redirects are disabled. A 3xx response fails instead of changing origin.
- TLS uses the bundled rustls/web-PKI stack and normal certificate validation.
- Connect timeout is 10 seconds and total request timeout is 120 seconds.

These controls limit DNS rebinding and origin changes within a request/client
lifetime. They do not make plaintext private-network traffic confidential, and
they do not protect against a compromised endpoint, local network, DNS resolver,
OS trust store, or same-user malware.

## Verification and protocol limits

Verification performs both required checks:

1. `GET /v1/models` must return JSON whose `data` includes the exact configured
   upstream model ID.
2. `POST /v1/chat/completions` requests a four-token, non-streaming test reply;
   the response must contain non-empty assistant content.

The subsequent chat request uses `stream: true` and accepts
`text/event-stream` lines containing OpenAI `choices[].delta.content`, ending
with `data: [DONE]`. JSON verification bodies are capped at 1 MiB; an SSE
response at 8 MiB; a single SSE line at 256 KiB; and assembled answer text at
1 MiB. Authentication/upstream bodies and parser errors are not reflected to
the terminal.

The current client intentionally targets the Chat Completions subset. Provider
extensions, tool calls, reasoning fields, multimodal messages, Responses API,
custom headers, proxy configuration, and alternative authentication schemes are
not supported yet.

## Dependency boundary

Remote HTTP uses pinned Reqwest 0.12.23 with rustls, Futures Util 0.3.34, and
Tokio 1.53.1 for a worker-local cancellable transport. The terminal uses pinned
Ratatui 0.29.0 with Crossterm 0.28.1 and Unicode Width 0.2.0. Credential
integration uses pinned keyring 3.6.3: native Apple/Windows stores and the pure-Rust async Secret
Service transport on Linux. The pure-Rust Linux choice avoids requiring users to
install development `libdbus` packages. Versions are selected for Rust 1.85 and
locked transitively; audits and target-platform runtime tests remain release gates.

Relevant upstream references: [Reqwest blocking client](https://docs.rs/reqwest/0.12.23/reqwest/blocking/),
[URL parsing](https://docs.rs/url/2.5.4/url/), and
[keyring entry API](https://docs.rs/keyring/3.6.3/keyring/struct.Entry.html).
