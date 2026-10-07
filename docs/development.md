# Development

## Current implementation

The Dociler v1 implementation is complete across Milestones M1 through M10:
- **M1**: Rust Cargo workspace, single public `dociler` binary, shared platform diagnostics, CLI integration tests, and GitHub Actions CI.
- **M2**: OS paths, validated JSON settings, explicit no-clobber initialization, canonical workspace policy lookup, bounded memory-only sessions, and credential-store interface.
- **M3**: Verified remote OpenAI-compatible profiles, native OS keychain storage, streamed chat, cancellable HTTP, interactive terminal UI with onboarding, profile lifecycle, and health checks.
- **M4**: Read-only RAM/CPU/disk inventory, Lite/Pro preflight classification, immutable revision-pinned asset manifest, SHA-256 verification, consent-gated downloader with byte-range resume, safe in-process runtime archive extraction with per-file inventory, and authenticated loopback runtime and model load probes with process-group peak RSS accounting.
- **M5**: Document processing pipeline: format detection and canonical AST across 7 formats (MD, TXT, DOCX, ODT, RTF, PDF, DOC), out-of-process sandboxed extraction worker (`dociler __worker-extract`), in-memory BM25 index with semantic block chunking, context strategies (Direct, Retrieval, Map/Reduce), diff-previewed in-place text editing, and multi-format export.
- **M6**: Terminal slash commands (`/permissions`, `/turn-on-remote`, `/turn-off-remote`, `/export`, `/undo`), `@file` and command completion, authenticated LAN gateway API server with token rotation, and CLI parity subcommands.
- **M7**: Embedded immutable versioned Markdown skill modules, deterministic task-relevant skill selection, 5-layer prompt hierarchy with subordinate client preferences, and transparent model profile aliasing (`dociler-lite`, `dociler-pro`).
- **M8**: Local and remote API behavior: Dociler-managed loopback `llama-server` router lifecycle on ephemeral ports with internal auth keys, gateway concurrency rate limiting (HTTP 429), `/v1/chat/completions` parameter bounds and SSE streaming, `/v1/documents/analyze` multipart pipeline with sandboxed extraction and RAII cleanup, destination-bound remote document consent gate, and OpenAPI 3.1 specification.
- **M9**: Privacy guarantees (zero telemetry, strictly ephemeral in-memory storage of chats/prompts/indexes/extracted text, mode `0o600` staging files), golden document test suite for all 7 formats with Indonesian/Unicode coverage and hostile/negative fixtures, adversarial prompt injection defense with delimiter sanitization, citation hallucination prevention, and formal model quality gate evaluations.
- **M10**: Native user-local standalone installers (`install.sh` for macOS/Linux, `install.ps1` for Windows), automated release packaging pipeline (`scripts/package_release.sh`), `NOTICE` attribution file, and installer integration test suite.

See [Configuration](configuration.md) and [Remote providers](remote-providers.md) for commands and boundaries.

Direct dependencies are pinned: directories 6.0.0, Serde 1.0.219, serde_json
1.0.140, tempfile 3.19.1, zeroize 1.8.1, URL 2.5.4, Reqwest 0.12.23,
keyring 3.6.3, Ratatui 0.29.0, Crossterm 0.28.1, Unicode Width 0.2.0,
SHA-2 0.10.9, and sysinfo 0.33.1 with only system/disk features.
Futures Util 0.3.34 and Tokio 1.53.1 support cancellable remote I/O without a
process-global runtime. Tokio's signal feature also backs a short-lived CLI
probe cancellation listener. fs2 0.4.3 provides cross-platform advisory asset locks;
libc 0.2.189 is used only for Unix no-follow file opens. flate2 1.1.2 (Rust
backend), tar 0.4.44, and ZIP 2.4.2 (Deflate only) provide in-process bounded
runtime archive parsing. getrandom 0.3.4 supplies the ephemeral internal probe
key from the OS random source.
The lockfile pins transitives. These
are deliberate Rust-1.85-compatible baseline pins, not claims to be the latest
releases. Dependency vulnerability/license audits remain release work.

## Prerequisites

Install Rust through [rustup](https://rustup.rs/). The repository pins Rust
1.85.1 with rustfmt and Clippy in `rust-toolchain.toml`; rustup selects it when
running Cargo here. This is a reproducible foundation baseline supporting
edition 2024, not a claim to use the latest Rust. Update the pin deliberately as
future dependencies require, and keep the minimum supported Rust version in
`Cargo.toml` consistent.

A native linker is also required: a C toolchain on Linux, Xcode Command Line
Tools on macOS, or MSVC Build Tools with the Windows SDK on Windows. Python 3.11+
is used for development documentation checks only; the Dociler executable has no
Python dependency.

## Build and run

From the repository root:

```sh
cargo run --locked -- --help
cargo run --locked -- --version
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
cargo run --locked -- chat
cargo build --release --locked
```

The resulting executable is `target/release/dociler` on macOS/Linux and
`target/release/dociler.exe` on Windows. It can be run from another directory.
Bare invocation opens the interactive surface only when standard input and
output are terminals. It prints help in redirected/non-interactive use. Use
`chat [PROFILE]` to select a saved profile explicitly and `run PROFILE` for one
prompt supplied on standard input.

`doctor` reports a canonical workspace path, OS, architecture, and current
development capabilities. It reads directory metadata and validates saved
settings if present. It also runs the read-only hardware/preflight snapshot. It
does not inspect document contents, start services, persist settings, probe an
accelerator/runtime, or perform final model admission.
Its exit status means the diagnostic command completed, not that inference is
available. Help/version exit with 0; invalid arguments with 2; diagnostic,
configuration, and output errors with 1. Broken output pipes exit quietly.
`config init` writes defaults explicitly and without overwriting. A successful
`connect add` also saves a verified non-secret profile; when a key is supplied,
it writes the key only to the OS credential service. `connect key`, confirmed
`connect key-clear`, and confirmed `connect remove` provide scriptable profile
lifecycle operations; `connect check` re-verifies saved state, and `connect edit`
verifies endpoint/model changes before commit. Authenticated cross-origin edits
also require `--confirm-credential-destination`. Secrets still enter only through
`DOCILER_API_KEY` or the native credential manager.

`model status` and the hardware section of `doctor` are read-only snapshots.
They do not create the model/runtime directories, download assets, select an
accelerator, or certify a model. A positive tier result means only that the
future runtime load/generation probe may be attempted.

`model list` reads filesystem metadata for the two pinned GGUF files and the
current platform's runtime archive without hashing. `model verify [PROFILE]`
streams each selected cached file through SHA-256 and returns nonzero when a
model or the runtime is missing, unsafe, unreadable, or does not match. Omitting
`PROFILE` verifies both tiers. These commands never create, repair, remove,
extract, download, or execute an asset.

`model download PROFILE --confirm [--restart]` is the explicit asset mutation
path. It downloads only the built-in current-platform runtime and selected
model, resumes a verified-prefix partial by default, and publishes only after
exact size/SHA-256 checks. `--restart` removes only that managed partial before
starting again. It does not extract or execute the runtime. The transport has a
cooperative cancellation token; terminal signal wiring for this scriptable path
is still pending, while process termination leaves the partial resumable.

`model runtime-install --confirm` revalidates and safely extracts the pinned
current-platform archive, creates a per-file inventory, and publishes a private
versioned runtime directory. It does not run or probe `llama-server`. `model
list` reports metadata state for that directory; `model verify` additionally
rehashes inventoried runtime files when an install exists.

`model runtime-probe --confirm` rehashes all installed runtime files and then
executes only the pinned server, without a GGUF, in a private model-free
loopback router. It verifies the server build, health, authentication, empty
model list, and router state, then kills and reaps it. This is a short diagnostic
only. It does not persist a preferred backend, enable local chat, or expose the
internal port/key. SIGINT/SIGTERM on Unix and Ctrl+C/Ctrl+Break on Windows
request cooperative cancellation; the probe stops and reaps its child before
the CLI exits nonzero. Forced termination cannot guarantee cleanup.

`model load-probe PROFILE --confirm` first rehashes the exact pinned GGUF and
installed runtime, repeats a live RAM/CPU/disk preflight, and refuses even the
experimental 6–8 GB Lite state. It checks the pinned server version, rehashes
both inputs and repeats preflight again, then briefly runs one CPU-only model
with a 1,024-token context, one slot, capped threads/batches, a private key,
and an OS-selected loopback port. It verifies health, key enforcement, alias,
model path, and build, makes one fixed non-thinking 16-token-max generation,
then kills/reaps the child. No response content or raw diagnostics are printed
or saved. A successful diagnostic does not qualify the 8K/16K target contexts,
RSS limits, GPU acceleration, model accuracy, or local chat. The same CLI
signal handling requests cooperative cancellation, including while generation
is stalled. On Linux, the command reports the child `llama-server` process's
`VmHWM` high-water RSS if `/proc` is available; other platforms report it as
unavailable. This excludes the Dociler parent, descendants, parser/index work,
and GPU memory, so it is not the process-group release memory gate and enforces
no hard memory limit.

## Workspace boundaries

- `crates/dociler`: process entrypoint, CLI parsing/output, Ratatui/Crossterm
  terminal adapter, and executable/TUI integration tests.
- `crates/dociler-core`: shared services independent of TUI/HTTP. Diagnostics is
  accompanied by config, paths, workspace, session, credential, hardware, and
  pinned-asset/cache-verification modules.
  Add substantive modules as features arrive rather than
  empty placeholder crates for every subsystem.
- `scripts/check_docs.py`: development-only handoff document checks.
- `.github/workflows/ci.yml`: native build/test matrix and quality checks.

The workspace has a single lockfile, inherited package metadata, forbidden
unsafe Rust, and shared lints. Crates are not publishable during development.
Generated assets and private state belong in ignored directories; synthetic,
redistributable test fixtures can be committed in designated fixture directories.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo doc --workspace --no-deps --locked
cargo build --workspace --release --locked
python3 scripts/test_tui_pty.py target/debug/dociler  # POSIX/Linux integration
python3 scripts/check_docs.py
git diff --check
```

CI uses read-only repository permissions, a commit-pinned checkout action, no
stored checkout credentials, bounded timeouts, and cancellation of superseded
runs. It builds and executes on Linux x86-64/ARM64, macOS Intel/Apple Silicon, and
Windows x86-64. Runner labels are selected from the
[GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Hosted runner checks do not certify minimum OS versions or 8/16 GB hardware.
Linux native jobs also drive the built executable through a real pseudo-terminal
against an ephemeral loopback mock provider, covering first-run onboarding,
multi-turn streaming, saved-profile health checking, cancellation disconnect,
resize survival, and terminal cleanup. The harness sends no public requests and
uses an isolated config path.

There is no publishing workflow yet. Dependency vulnerability/license auditing,
release packages, checksums, signed manifests, SBOMs, provenance, and installer
tests must be added before any public release; they are not implied by a green
foundation CI run. Dependabot monitors future Cargo dependencies and Actions pins.

## Milestones and handoff

1. **M1**: Runnable Rust workspace, shared diagnostics, and reproducible CI foundation.
2. **M2**: Configuration and session state, canonical workspace identity, OS data paths, credential-store interface, and read-only permission defaults.
3. **M3**: Terminal onboarding and remote text-chat adapter with streaming and ephemeral history.
4. **M4**: Hardware admission, verified local asset management, and runtime/model load diagnostics.
5. **M5**: Isolated document extraction worker, canonical AST across 7 formats, in-memory BM25 retrieval, context strategies, in-place text editing, and multi-format export.
6. **M6**: Terminal slash commands (`/permissions`, `/turn-on-remote`, `/turn-off-remote`, `/export`, `/undo`), `@file` completion, authenticated LAN gateway, and CLI parity subcommands.
7. **M7**: Embedded immutable Markdown skills, deterministic task-relevant selection, 5-layer prompt assembly, and model profile aliasing.
8. **M8**: Local and remote API behavior: managed loopback `llama-server` router lifecycle, gateway concurrency rate limiting (HTTP 429), `/v1/chat/completions`, multipart `/v1/documents/analyze`, remote document consent gate, and OpenAPI 3.1.
9. **M9**: Privacy guarantees (zero telemetry, non-persistence), golden document fixtures across all 7 formats, Indonesian/Unicode test matrix, adversarial prompt injection defense, citation validation, and model quality gates.
10. **M10**: Native user-local installers (`install.sh` for macOS/Linux, `install.ps1` for Windows), release packaging pipeline (`scripts/package_release.sh`), `NOTICE` attribution, and installer integration test suite.

These milestones organize the approved v1 scope; they do not waive any release gate. Consult `CHECKPOINT.md` for actual progress and append a record after each meaningful unit of work.
