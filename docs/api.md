# API Contract

## Scope

Dociler v1 exposes a small OpenAI-compatible surface plus one document-analysis
endpoint. It is not a transparent `llama-server` proxy and does not claim full
OpenAI API compatibility.

Default LAN address after explicit enablement:

```text
http://0.0.0.0:11435
```

The default state is off. Dociler does not open firewalls, configure routers,
obtain public addresses, or provide internet TLS termination.

## Authentication and transport

- All `/v1/*` endpoints require `Authorization: Bearer <token>`.
- `/healthz` may return basic process health without authentication but must not
  reveal paths, model sources, prompts, files, or configuration.
- Generate a cryptographically random token when serving is first enabled and
  store it in the OS credential store.
- Support explicit token rotation, immediately invalidating the old token.
- Disable permissive CORS. Reject unexpected content types and oversized bodies.
- Limit local inference to one concurrent generation; queue a small bounded
  number of requests and return a standard 429 error when full.
- Recommend a trusted private network. Internet exposure requires a separately
  managed TLS/authentication reverse proxy and is outside v1.

## Error shape

Use one stable JSON error envelope:

```json
{
  "error": {
    "message": "Human-readable message",
    "type": "invalid_request_error",
    "code": "unsupported_document",
    "param": "file"
  }
}
```

Do not include stack traces, internal ports/tokens, absolute workspace paths,
upstream credentials, or raw parser/runtime logs.

## `GET /healthz`

Returns 200 when the gateway can accept a request, even if no model is loaded:

```json
{
  "status": "ok",
  "backend": "local",
  "model_status": "ready"
}
```

Allowed model states are `unloaded`, `loading`, `ready`, and `error`. Do not
return an upstream base-model ID.

## `GET /v1/models`

Returns only the active Dociler profile. A local Qwen GGUF filename or a remote
upstream model name must never leak through this endpoint.

```json
{
  "object": "list",
  "data": [
    {
      "id": "dociler-lite",
      "object": "model",
      "created": 0,
      "owned_by": "dociler"
    }
  ]
}
```

If no profile is ready, return an empty `data` array rather than inventing an
available model.

## `POST /v1/chat/completions`

Support the commonly used Chat Completions fields:

- `model` (required): must exactly match the active `dociler-lite` or
  `dociler-pro` alias;
- `messages` (required): text system/user/assistant messages;
- `stream` (optional, default `false`);
- `temperature`, `top_p`, `max_tokens`, and `stop` within server-enforced safe
  bounds.

Unsupported fields return a clear 400 response rather than being silently
forwarded. Built-in Dociler policy and relevant skills are prepended on every
request; client system messages cannot replace them.

Non-streaming responses use the standard chat-completion envelope. Streaming
uses `text/event-stream`, OpenAI-style `data:` chunks, a terminal `[DONE]`, and
prompt cancellation when the client disconnects.

Requests from this generic endpoint do not gain filesystem access. An external
client must include text in messages or use the document endpoint.

## `POST /v1/documents/analyze`

Accept `multipart/form-data` with:

| Field | Required | Meaning |
| --- | --- | --- |
| `file` | yes | One supported word-processing document. |
| `prompt` | yes | Question or transformation request. |
| `model` | no | Active Dociler alias; mismatch is rejected. |
| `stream` | no | `false` by default. |

The upload is processed in an isolated extractor worker and held in memory. It
is deleted from temporary storage in all success, failure, cancellation, and
disconnect paths. The endpoint is analysis-only and cannot write to the server's
workspace.

Return the same chat-completion envelope or SSE format as the chat endpoint.
Include validated source references in assistant text and optional
`dociler_sources` response metadata. Source metadata contains the uploaded
filename plus page/section anchors, never an absolute path.

Reject unsupported types, extension/MIME mismatches, encrypted content,
image-only PDFs, expansion bombs, and configured size/text/time-limit breaches.

## `GET /openapi.json`

Return a versioned OpenAPI 3.1 document describing only supported public routes,
schemas, authentication, size limits, response examples, and error codes. The
specification is contract-tested against the router.

## Compatibility tests

- List and call the active alias with an official OpenAI SDK configured to the
  Dociler base URL.
- Stream and cancel a response with a second independent compatible client.
- Verify missing/invalid tokens produce 401 without useful side channels.
- Verify the API never exposes the raw model ID, raw child port, skills text,
  workspace paths, or credentials.
- Verify client disconnect cancels generation and cleans uploaded data.
