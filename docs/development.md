# Development

## Current implementation

M1 provides a Cargo workspace, a basic executable, shared diagnostics, CLI
integration tests, and GitHub Actions. M2 adds OS paths, validated JSON settings,
explicit no-clobber initialization, canonical workspace policy lookup, bounded
memory-only sessions, and a credential-store interface. M3 adds verified remote
profiles, OS keychain integration, streamed one-shot chat, a provider-independent
generation controller, cancellable HTTP, and an initial interactive multi-turn
terminal surface with remote onboarding. Local onboarding, inference, document
processing, and serving remain future work. See
[Configuration](configuration.md) and
[Remote providers](remote-providers.md) for commands and boundaries.

Direct dependencies are pinned: directories 6.0.0, Serde 1.0.219, serde_json
1.0.140, tempfile 3.19.1, zeroize 1.8.1, URL 2.5.4, Reqwest 0.12.23,
keyring 3.6.3, Ratatui 0.29.0, Crossterm 0.28.1, and Unicode Width 0.2.0.
Futures Util 0.3.34 and Tokio 1.53.1 support cancellable remote I/O without a
process-global runtime.
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
cargo run --locked -- connect list
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
settings if present. It does not inspect document contents, start services,
persist settings, or test memory/GPU readiness.
Its exit status means the diagnostic command completed, not that inference is
available. Help/version exit with 0; invalid arguments with 2; diagnostic,
configuration, and output errors with 1. Broken output pipes exit quietly.
`config init` writes defaults explicitly and without overwriting. A successful
`connect add` also saves a verified non-secret profile; when a key is supplied,
it writes the key only to the OS credential service.

## Workspace boundaries

- `crates/dociler`: process entrypoint, CLI parsing/output, Ratatui/Crossterm
  terminal adapter, and executable/TUI integration tests.
- `crates/dociler-core`: shared services independent of TUI/HTTP. Diagnostics is
  accompanied by config, paths, workspace, session, and credential modules.
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
python3 scripts/check_docs.py
git diff --check
```

CI uses read-only repository permissions, a commit-pinned checkout action, no
stored checkout credentials, bounded timeouts, and cancellation of superseded
runs. It builds and executes on Linux x86-64/ARM64, macOS Intel/Apple Silicon, and
Windows x86-64. Runner labels are selected from the
[GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Hosted runner checks do not certify minimum OS versions or 8/16 GB hardware.

There is no publishing workflow yet. Dependency vulnerability/license auditing,
release packages, checksums, signed manifests, SBOMs, provenance, and installer
tests must be added before any public release; they are not implied by a green
foundation CI run. Dependabot monitors future Cargo dependencies and Actions pins.

## Milestones and handoff

1. M1: runnable Rust/CI foundation and reproducible checks.
2. M2: configuration and session state, canonical workspace identity, OS data
   paths, credential-store interface, and read-only permission defaults.
3. M3: terminal onboarding and remote text-chat adapter with streaming and
   ephemeral history, followed by document consent when document input arrives.
4. M4: hardware admission, verified local asset management, runtime integration.
5. M5: isolated extraction, retrieval, skills, and grounded document workflows.
6. M6: controlled editing/exports, authenticated API, and background lifecycle.
7. M7: installers, packaging, platform/security checks, and model qualification.

These milestones organize the approved v1 scope; they do not waive any release
gate. Consult `CHECKPOINT.md` for actual progress and append a record after each
meaningful unit of work.
