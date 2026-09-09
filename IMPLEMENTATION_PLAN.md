# Dociler v1 Implementation Plan

This is the approved, decision-complete plan. It describes future work; none of
the application behavior below is implemented in the current documentation-only
repository.

## 1. Fixed product decisions

- Build a privacy-focused terminal document assistant named **Dociler** with the
  executable and configuration namespace `dociler`.
- Support macOS 13+ on Apple Silicon and Intel, Linux glibc on x86-64 and ARM64,
  and Windows 10/11 on x86-64.
- Build the application in Rust and use native installation by default. Docker
  distribution is deferred until after native v1 is stable.
- Use a Dociler-managed `llama.cpp` sidecar for local inference, initially pinned
  to v0.4.0 (`5266f24`). The sidecar must never be treated as a separately
  installed system prerequisite.
- Use Qwen3.5 4B Q4_K_M for `dociler-lite` and Qwen3.5 9B Q4_K_M for
  `dociler-pro`, subject to the qualification gates in this plan.
- License Dociler under Apache-2.0.
- Support built-in, versioned document skills only in v1. Do not load global or
  workspace-provided skills.
- Keep sessions, extracted text, and retrieval indexes in memory by default.

## 2. Repository and build foundation

The legacy implementation is preserved at `archive/legacy-ollama`. Scaffold a
Rust workspace that produces one public `dociler` executable. Internally, the
executable may relaunch itself in a hidden extraction-worker mode so malformed
documents cannot crash the interactive process.

Use clear internal boundaries for:

- configuration and OS credential storage;
- hardware detection and model/runtime management;
- document extraction and canonical document representation;
- chunking, retrieval, prompt construction, and citation validation;
- local and remote inference adapters;
- chat/session orchestration;
- terminal UI and non-interactive commands;
- authenticated HTTP serving and lifecycle management;
- permission enforcement, editing, and export.

Replace the deleted GitLab pipeline with GitHub Actions when implementation is
authorized. CI must cover formatting, linting, unit tests, dependency/license
audits, cross-platform builds, checksums, SBOMs, and release provenance.

## 3. Installation and onboarding

Provide a repository-level `install.sh` for macOS/Linux and `install.ps1` for
Windows. The scripts download and verify a prebuilt Dociler release into a
user-local binary directory. They must not install Docker, Python, Ollama,
models, or system-wide services.

On the first `dociler` launch:

1. Display the current workspace, detected RAM, available memory, CPU/GPU
   acceleration, free disk, and supported document count.
2. Ask whether to use a local model or an OpenAI-compatible remote endpoint.
3. For local mode, present Lite as the recommended 8 GB-class option and Pro as
   the higher-quality 16 GB-class option.
4. Before downloading, show source, license, exact size, required free space,
   expected memory class, and checksum.
5. Download the matching pinned `llama.cpp` backend and model with resume,
   cancellation, atomic completion, and SHA-256 verification.
6. Start the local runtime on loopback, verify health, and enter chat.
7. For remote mode, collect the base URL, optional API key, and upstream model;
   validate `/v1/models` and a minimal generation request before entering chat.

Subsequent launches show the last profile with obvious controls to continue,
switch local models, or connect remotely. Update checks may notify periodically,
but installation requires explicit confirmation.

## 4. Model and runtime profiles

The initial pinned artifacts are:

| Profile | Upstream artifact | Size | SHA-256 | Active context |
| --- | --- | ---: | --- | ---: |
| `dociler-lite` | `bartowski/Qwen_Qwen3.5-4B-GGUF` / `Qwen_Qwen3.5-4B-Q4_K_M.gguf` | 3.01 GB | `13c16f426047e2de38cd075bdade4a7bcbc8c774384876f677740cda65f8a983` | 8,192 |
| `dociler-pro` | `bartowski/Qwen_Qwen3.5-9B-GGUF` / `Qwen_Qwen3.5-9B-Q4_K_M.gguf` | 6.17 GB | `d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43` | 16,384 |

Do not download the vision projector in v1. Use one inference slot, automatic
thread selection, automatic safe GPU offload, flash attention where validated,
and non-thinking document responses by default. Dynamically reduce context or
recommend remote mode when available memory is insufficient.

Lite is supported on an 8 GB machine and may be offered experimentally on a
6–8 GB machine only after a live admission check. Below 6 GB, recommend remote
mode. Pro targets a 16 GB machine and must not be advertised as a reliable local
option merely because the model file can be memory-mapped.

Before stable release, compare each primary model against its predetermined
fallback: Qwen3 4B Instruct 2507 Q4_K_M for Lite and Qwen3 8B Q4_K_M for Pro.
Use a fallback only if the Qwen3.5 candidate fails a stated memory or quality
gate. If the fallback also fails, block that tier's stable release.

## 5. Document processing

Read `.md`, `.pdf`, `.docx`, `.doc`, `.rtf`, `.odt`, and `.txt`. Put Extractous
behind a Dociler-owned parser interface and isolate each parse in a constrained
worker process. Parse into a canonical AST containing metadata, headings,
paragraphs, lists, tables, links, page/section anchors, and source locations.

Text PDFs are supported. Detect image-only/scanned and password-protected PDFs
and explain why they cannot be read. OCR is an optional future pack.

Build an in-memory, paragraph-aware chunk index using the selected model's
tokenizer and BM25. Preserve heading/page provenance on every chunk. Use direct
context for small selections, retrieval for questions, and deterministic
map/reduce summarization for long or multiple documents. Validate all cited
source identifiers before rendering them.

Workspace discovery must respect `.gitignore`, exclude hidden/build/vendor
directories by default, refuse symlinks escaping the workspace, and enforce
file-size, decompression, extracted-text, memory, and time limits. Documents are
untrusted data and cannot override system or built-in skill instructions.

## 6. Terminal experience and permissions

Build an immersive but learnable TUI with a scrollable Markdown transcript,
streaming output, visible current backend/model/workspace/context state,
download and indexing progress, keyboard cancellation, and actionable errors.
Support `@file` completion and a document picker.

Required slash commands are `/help`, `/model`, `/connect`, `/files`, `/status`,
`/clear`, `/permissions`, `/export`, `/turn-on-remote`, `/turn-off-remote`,
`/update`, and `/exit`. Provide equivalent scriptable subcommands for doctor,
model management, connection management, serving, updates, and one-shot prompts.

Workspace access is read-only by default. A write grant is persisted per
canonical workspace path, but every write still requires a preview and explicit
confirmation. Permit atomic in-place edits only for `.md` and `.txt`, with
session-memory undo. Permit new Markdown, TXT, DOCX, RTF, and ODT exports from
the canonical AST. Never overwrite PDF, DOC, DOCX, RTF, or ODT sources; legacy
`.doc` is input-only.

## 7. Skills and prompt construction

Ship compact, immutable Markdown skill modules for discovery, grounded reading,
summarization, comparison, citation, and safe editing. Embed and version them
with the release, select only task-relevant modules, and inject them into every
CLI and gateway request. Client system messages are subordinate to Dociler's
core policy.

`dociler-lite` and `dociler-pro` are transparent application profiles, not newly
trained weights. Expose only these aliases in Dociler's API, while crediting the
base model in `/model info`, notices, and documentation.

## 8. Local and remote API behavior

Run `llama-server` on a random loopback port with a generated internal API key.
Neither its raw port nor base-model identifier is part of the public contract.
The Dociler gateway injects built-in skills, enforces aliases and limits, and
normalizes errors and streaming.

The first public surface is:

- `GET /healthz`
- `GET /v1/models`
- `POST /v1/chat/completions`
- `POST /v1/documents/analyze`
- `GET /openapi.json`

`/turn-on-remote` binds the gateway to `0.0.0.0:11435`, generates a bearer token,
disables permissive CORS, and does not alter firewalls or routers. On TUI exit,
ask whether to stop the server or retain it as a per-user background service.
`/turn-off-remote` stops that service. Support token rotation.

For upstream remote profiles, allow HTTP only for loopback or private-network
destinations and require HTTPS elsewhere. Store API keys in the OS keychain.
Before sending any local document content upstream, identify the destination
and obtain consent once per session.

## 9. Privacy, tests, and release gates

Do not add telemetry. Do not persist chat, extracted text, prompts, retrieval
indexes, or document copies unless the user explicitly exports a transcript or
document. Store only non-sensitive preferences and approved workspace paths in
configuration.

Testing must include unit coverage, golden documents for every format,
Unicode/Indonesian content, malformed and hostile inputs, path-containment and
prompt-injection scenarios, API conformance, remote consent, editing/export
safety, clean installer runs, and the complete supported platform matrix.

Stable model gates:

- Lite: at least 85% factual QA accuracy and 95% valid citations.
- Pro: at least 92% factual QA accuracy and 97% valid citations.
- Zero invented numeric/date values in the deterministic extraction suite.
- Lite: sustained real session at 8K context on an 8 GB host with process-group
  peak RSS no greater than 5.5 GiB.
- Pro: sustained real session at 16K context on a 16 GB host with process-group
  peak RSS no greater than 11.5 GiB.

## 10. Explicitly deferred

Do not include spreadsheets, presentations, browser/web ingestion, Google Docs
or MCP, OCR, persistent RAG/vector databases, custom user/project skills,
arbitrary shell execution, general coding-agent behavior, in-place binary Office
editing, or Docker distribution in v1.
