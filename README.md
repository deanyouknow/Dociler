# Dociler

Dociler is a local-first terminal assistant in development for reading, searching,
comparing, summarizing, and drafting universal documents with either a local
LLM or a user-configured OpenAI-compatible API.

> **Project status:** Implementation has begun. The Rust CLI foundation provides
> help, version, workspace diagnostics, and validated settings with explicit
> initialization. Remote OpenAI-compatible profile verification and streamed
> one-shot text chat are available. An initial memory-only, multi-turn terminal
> interface, remote-profile onboarding, refresh and connection checks, verified
> endpoint/model editing, confirmed profile removal, and verified key rotation
> are also available. Read-only RAM/CPU/disk inventory and conservative Lite/Pro
> preflight reporting are available through `model status`. A built-in,
> revision-pinned asset manifest and read-only cache listing/SHA-256 verification
> are available through `model list` and `model verify`. Local model/runtime
> download, extraction, and execution, local backend onboarding, and document
> analysis are not implemented yet.

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

## Intended installation experience

The eventual repository will provide `install.sh` for macOS/Linux and
`install.ps1` for Windows. Installation will place only the Dociler executable
and release metadata. A compatible `llama.cpp` runtime and the selected model
will be downloaded later, only if the user chooses local mode during onboarding.

The installer is not implemented yet. Developers can run the CLI foundation:

```sh
cargo run --locked -- --help
cargo run --locked -- doctor
cargo run --locked -- config paths
cargo run --locked -- config show
cargo run --locked -- model status
cargo run --locked -- model list
cargo run --locked -- model verify dociler-lite
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
