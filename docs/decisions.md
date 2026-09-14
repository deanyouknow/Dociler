# Architecture Decisions

This file records decisions already made. Changing one requires an explicit new
entry, updates to affected documents, and a checkpoint note.

## ADR-001: Native Rust application

**Decision:** Build Dociler as a native Rust CLI/TUI.

**Why:** The product needs low idle overhead, single-command cross-platform
distribution, reliable process/lifecycle control, and an immersive terminal UI.

**Rejected:** A Python application would accelerate prototyping but adds runtime
and packaging variability; Node/Bun adds another runtime and less direct native
integration. These may still be used for development utilities, not the shipped
runtime, if they do not become user prerequisites.

## ADR-002: `llama.cpp` over Ollama

**Decision:** Manage a pinned `llama.cpp` runtime as an internal sidecar.

**Why:** It gives Dociler direct control over GGUF files, context, quantization,
GPU offload, concurrency, bind address, authentication, model aliases, process
lifecycle, and platform-specific packaging with lower overhead.

**Rejected:** Ollama offers excellent convenience but introduces a separately
managed daemon and model registry/API whose lifecycle and model visibility do
not match Dociler's gateway contract.

## ADR-003: Native-first, no v1 Docker distribution

**Decision:** Ship native installation first and defer Docker.

**Why:** Docker Desktop adds VM/storage/RAM overhead on consumer macOS/Windows
devices and complicates accelerator access. The target experience is a direct
`dociler` command on resource-constrained laptops.

**Rejected:** Docker remains a potential later headless/server distribution once
the shared core and API stabilize.

## ADR-004: Qwen3.5 4B/9B primary profiles

**Decision:** Qualify Qwen3.5 4B Q4_K_M as Lite and Qwen3.5 9B Q4_K_M as Pro.

**Why:** They provide current multilingual, instruction-following, long-context,
and document-extraction capability at quantized sizes appropriate for the chosen
hardware classes.

**Constraint:** They are candidates until they pass explicit quality and memory
gates. Predetermined Qwen3 4B Instruct 2507 and Qwen3 8B fallbacks prevent ad hoc
model selection. The aliases must remain transparent profiles, not claims of
newly trained models.

## ADR-005: Retrieval without embeddings in v1

**Decision:** Use semantic block chunking plus in-memory BM25 and deterministic
map/reduce.

**Why:** It avoids another model download and memory consumer, preserves privacy,
supports exact term/numeric lookup, and keeps the LLM context bounded.

**Rejected:** Persistent vector storage and embedding models may be evaluated
later against measured recall, RAM, disk, and privacy costs.

## ADR-006: Extractous behind an isolated interface

**Decision:** Use Extractous for the broad v1 input set, wrapped in a
Dociler-owned interface and a constrained worker process.

**Why:** It covers PDF, modern/legacy Word, RTF, ODT, Markdown, and text with a
native integration. Isolation limits crashes and hostile parser inputs and keeps
the rest of the application independent from its types.

**Rejected:** MarkItDown does not provide the required legacy DOC path cleanly;
Docling and Java Tika services are heavier for the target machines; one custom
parser per format creates excessive maintenance and inconsistent output.

## ADR-007: Immutable built-in skills only

**Decision:** Embed compact, versioned document skills and inject relevant
modules on every request.

**Why:** Request-time injection applies equally to local, remote, TUI, and API
use and can be tested/versioned independently of the GGUF. Restricting v1 to
built-ins limits precedence, trust, and token-budget complexity.

**Rejected:** Baking only a Modelfile system prompt cannot enforce behavior
through every gateway path. User/global/project skills are deferred.

## ADR-008: Read-only and ephemeral defaults

**Decision:** Default to read-only workspaces and memory-only chats, extraction,
and retrieval indexes.

**Why:** The originating use case includes sensitive documents. Persistent write
grants are per workspace, but every operation remains previewed and confirmed.
Only text sources are edited in place; binary formats produce new exports.

## ADR-009: Small authenticated API surface

**Decision:** Expose models, chat completions, a single document-analysis route,
health, and OpenAPI metadata through a Dociler gateway.

**Why:** This covers common harness connections while keeping policy injection,
alias filtering, file handling, and authentication under Dociler's control.

**Rejected:** A catch-all proxy would leak unsupported behavior and the base
runtime. Responses, Files, embeddings, and broad OpenAI emulation are deferred.

## ADR-010: LAN serving is explicit and token-protected

**Decision:** `/turn-on-remote` enables bearer-authenticated LAN access without
firewall/router automation; exit asks whether the service should remain.

**Why:** It supports another harness while making exposure visible and bounded.
Public internet serving requires TLS and multi-user controls beyond v1.

## ADR-011: Documentation-driven checkpoints

**Decision:** Every meaningful progress unit updates root `CHECKPOINT.md` under
the protocol in `AGENTS.md`.

**Why:** The project may pass between AI agents. A canonical current-state and
append-only activity record prevents chat context from becoming the only source
of truth and makes incomplete verification visible.

## ADR-012: Strict settings and memory-only session foundation

**Decision:** Use a bounded schema-v1 JSON file in the OS-local configuration
directory, with explicit no-clobber initialization, unknown-field rejection, and
exact canonical workspace grant lookup. Keep session messages and credentials
out of serializable configuration types. Native credential adapters are a
separate next-stage integration and must not fall back to plaintext files.

**Why:** A small, testable schema gives safe missing-file defaults while making
invalid security settings visible. OS-local paths avoid roaming write grants;
explicit initialization avoids startup writes. JSON/Serde supplies structured
validation without a custom settings parser. See [Configuration](configuration.md).

**Rejected:** Workspace-local settings auto-loading, silently resetting corrupt
settings, unbounded persistent history, and permissive keychain fallbacks.

## ADR-013: Pinned remote origin and native-only credentials

**Decision:** Build the first upstream adapter with Reqwest/rustls, no ambient
proxy, no redirects, and DNS addresses pinned for each client lifetime. Require
HTTPS publicly and allow HTTP only when all resolved addresses are private or
loopback. Store optional keys through keyring-backed native credential services;
use a pure-Rust Secret Service transport on Linux.

**Why:** The same remote flow must eventually carry sensitive documents. Origin
validation must precede that feature, and provider keys must never need a
plaintext compatibility fallback. Model listing plus minimal generation catches
more incompatible endpoints than a health probe alone. See
[Remote providers](remote-providers.md).

**Rejected:** Automatic redirects, environment-proxy inheritance, public HTTP,
URL-embedded credentials, API keys in JSON, and Linux integrations requiring a
development `libdbus` package.

## ADR-014: Native event-loop TUI with isolated blocking generation

**Decision:** Implement the first terminal surface with pinned Ratatui 0.29 and
Crossterm 0.28, preserving separate non-interactive commands. Keep terminal
events/rendering on the main thread and move each blocking remote generation to
a worker thread connected by an in-process channel. Hold transcript, prompt,
partial output, and model session only in memory, with best-effort zeroization.

**Why:** These versions retain the Rust 1.85 baseline, support deterministic
off-screen rendering tests, and restore terminal state through a small native
adapter. A background worker keeps streamed text and cancellation input
responsive without introducing an async runtime before the broader application
orchestrator exists. Refusing the TUI when stdin/stdout are not terminals keeps
automation output stable and free of control sequences.

**Rejected:** Replacing scriptable commands with a terminal-only UI, persisting
transcripts, silently falling back from failed TUI setup, and introducing a
browser/Electron frontend. The current cooperative cancellation is an interim
M3 boundary, not a rejection of transport-level abort handling.
