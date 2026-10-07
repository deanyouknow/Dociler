# Dociler

Dociler is a local-first terminal assistant in development for reading, searching,
comparing, summarizing, and drafting universal documents with either a local
LLM or a user-configured OpenAI-compatible API.

> **Project status:** v1 implementation is complete across Milestones M1 through
> M10. The native Rust executable (`dociler`) provides full document processing for
> 7 supported formats (MD, TXT, DOCX, ODT, RTF, PDF, DOC), out-of-process sandboxed
> extraction, in-memory BM25 indexing, 5-layer prompt assembly with adversarial
> injection defense, embedded immutable skills, terminal interactive chat with
> slash commands and @file completion, controlled in-place editing and multi-format
> export, authenticated LAN gateway API (/v1/chat/completions, /v1/documents/analyze,
> OpenAPI 3.1), managed loopback llama-server router lifecycle with concurrency
> rate limiting, strict privacy guarantees (zero telemetry, zero persistence of
> chats/prompts/indexes), model quality qualification gates, and user-local native
> installers (`install.sh`, `install.ps1`). Local model chat remains qualification-gated
> pending full-context memory and model quality certification.

The first release is scoped to word-processing documents: Markdown (`.md`),
PDF (`.pdf`), modern and legacy Word (`.docx`, `.doc`), Rich Text Format
(`.rtf`), OpenDocument Text (`.odt`), and plain text (`.txt`). Spreadsheets,
presentations, web ingestion, Google Docs/MCP, OCR, and arbitrary shell access
are explicitly deferred.

## Product direction

- A native `dociler` CLI/TUI for macOS, Linux, and Windows.
- Read-only workspace analysis by default, with controlled writing enabled per
  workspace and confirmed before each write.
- Local inference through a Dociler-managed `llama.cpp` process.
- `dociler-lite`, based initially on Qwen3.5 4B Q4_K_M, for 8 GB-class devices.
- `dociler-pro`, based initially on Qwen3.5 9B Q4_K_M, for 16 GB-class devices.
- Remote inference through a user-supplied OpenAI-compatible endpoint.
- An optional authenticated OpenAI-compatible LAN API for other harnesses.
- Ephemeral chat, extracted text, and search indexes by default.

The RAM classes are supported targets, not guarantees for every machine below
those capacities. Model qualification and measured memory limits are release
gates described in the testing documentation.

## Installation

Dociler provides user-local installation scripts for macOS, Linux, and Windows.
Installation places only the standalone `dociler` executable into your user
binary directory (`~/.local/bin` on POSIX systems, `%LOCALAPPDATA%\Programs\Dociler\bin` on Windows).
It never installs Docker, Python, Ollama, model weights, or system-wide services.

### macOS and Linux

```sh
curl -fsSL https://raw.githubusercontent.com/deanyouknow/Dociler/main/install.sh | sh
```

Or run `install.sh` locally:
```sh
./install.sh --help
./install.sh --dry-run
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/deanyouknow/Dociler/main/install.ps1 | iex
```

Or run `install.ps1` locally:
```powershell
.\install.ps1 -Help
.\install.ps1 -DryRun
```

Developers can run the CLI from source:

```sh
cargo run --locked -- --help
cargo run --locked -- doctor
cargo run --locked -- config paths
cargo run --locked -- config show
cargo run --locked -- model status
cargo run --locked -- model list
cargo run --locked -- model verify dociler-lite
cargo run --locked -- model download dociler-lite --confirm
cargo run --locked -- model runtime-install --confirm
cargo run --locked -- model runtime-probe --confirm
cargo run --locked -- model load-probe dociler-lite --confirm
cargo run --locked -- connect list
cargo run --locked -- connect check PROFILE
cargo run --locked -- connect edit PROFILE URL MODEL
cargo run --locked -- connect remove PROFILE --confirm
cargo run --locked -- chat
```

See [Development](docs/development.md) for toolchain setup, builds, and checks.
See [Remote providers](docs/remote-providers.md) for profile setup and both chat
paths. In a terminal, bare `dociler` and `dociler chat [PROFILE]` open the same
interactive screen; redirected bare invocation safely prints help.

## Documentation

- [Approved implementation plan](IMPLEMENTATION_PLAN.md)
- [Current project checkpoint](CHECKPOINT.md)
- [Agent handoff and conversation context](HANDOFF.md)
- [Instructions for AI agents and contributors](AGENTS.md)
- [Product requirements](docs/product-requirements.md)
- [System architecture](docs/architecture.md)
- [CLI and terminal experience](docs/cli-ux.md)
- [API contract](docs/api.md)
- [Document processing pipeline](docs/document-pipeline.md)
- [Models and inference runtime](docs/models-and-runtime.md)
- [Security and privacy](docs/security-and-privacy.md)
- [Testing and release gates](docs/testing-and-release.md)
- [Architecture decisions](docs/decisions.md)
- [Development and implementation milestones](docs/development.md)
- [Configuration reference](docs/configuration.md)
- [Remote providers and text chat](docs/remote-providers.md)

## Resuming development

Before making changes, read `AGENTS.md`, `IMPLEMENTATION_PLAN.md`, and
`CHECKPOINT.md`, followed by the documents relevant to the task. Implementation
has been authorized; the checkpoint tracks the current milestone and next task.
For the agent transition snapshot and conversation context, also read
[HANDOFF.md](HANDOFF.md); it supplements, rather than replaces, the checkpoint.

Every agent that makes meaningful progress must update `CHECKPOINT.md` before
handing off. The checkpoint is the canonical answer to “what is done, what was
verified, and what happens next?”

## Legacy implementation

The abandoned Ollama/FastAPI/Docker implementation was archived before this
documentation reset. It is recoverable from the annotated Git tag
`archive/legacy-ollama` at commit
`c0f9c1f5236820fd5d9c717c80e7f44838a3d195`.

## License

Dociler is licensed under Apache-2.0. See [LICENSE](LICENSE).
