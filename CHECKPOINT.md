# Project Checkpoint

Last updated: 2026-09-11T16:54:06Z

## Current status

**Status:** M3 remote-provider integration in progress; earlier hosted CI pending.

**Active milestone:** M3 — Remote endpoint and text-chat integration.

The user authorized implementation on 2026-09-11. The workspace now builds a
`dociler` executable with help, version, workspace/platform diagnostics, config
inspection, and explicit default-settings initialization. It includes canonical
workspace policy lookup, bounded memory-only session types, and a credential
interface. It has no TUI, chat, document parsing, model/runtime integration,
native credential adapter, settings/grant update UI, or API yet. The five-platform
CI workflow is defined but has not been run on GitHub from this environment.

## Completed

- Archived the abandoned Ollama/FastAPI/Docker implementation at annotated Git
  tag `archive/legacy-ollama`, targeting commit
  `c0f9c1f5236820fd5d9c717c80e7f44838a3d195`.
- Removed all legacy tracked application, Docker, Ollama, gateway, skills, setup,
  health-check, ignore, and GitLab CI files from the working tree.
- Created the Dociler root handoff documents and detailed `/docs` package.
- Established the mandatory checkpoint protocol for future agents.
- Added two Rust workspace packages (`dociler`, `dociler-core`), shared metadata
  and lints, a lockfile, pinned Rust 1.85.1 toolchain, and release build profile.
- Implemented CLI help/version/doctor with explicit development capability
  reporting, non-interactive help, safe argument errors, and quiet broken pipes.
- Added six executable/service integration tests (five portable, one Unix-only).
- Added GitHub Actions quality/native build checks and Dependabot configuration.
- Added persistent documentation checks, development instructions, and ignores
  for build outputs, local state, model weights, and private exports.
- Added M2 core modules for OS paths, strict schema-v1 JSON settings, exact
  canonical workspace grants, bounded ephemeral sessions, and redacted secrets.
- Added `config paths`, `config show`, and `config init`; doctor now validates
  settings and reports current policy. Inspection creates no state. Initialization
  publishes a private default file without overwriting existing work.
- Added 14 tests for M2 (20 total on Linux), including malformed/oversized config,
  concurrent initialization, privacy, symlinks, permissions, workspace identity,
  session limits/clearing, and unavailable credential-store behavior.
- Rebuilt and smoke-tested the Linux release executable; documented the schema,
  commands, limitations, pinned dependencies, and ADR-012.

## Work in progress

Implementing remote profile validation, endpoint/DNS/redirect policy, native
credential storage, OpenAI-compatible verification and text streaming, and
mock-server tests. Starting tree is clean. M1/M2 hosted CI/native execution
remains unverified.

## Next recommended task

Run the CI workflow when these changes reach GitHub. Begin M3 with remote profile
and endpoint validation, native credential-store integration, and mock-server
tests; then connect onboarding and streaming remote text chat to the session
container. Implement HTTPS/private-address rules and redirect policy before
sending user content. Keep remote secrets out of settings and make missing OS
keychain availability explicit. Document transmission/consent and local models
remain later work; no model downloads are needed for this next task.
Implementation is already authorized; no additional product decision is needed.

## Blockers

None.

Environment note: Rust was absent, so verification used isolated development
tools at `/tmp/dociler-cargo` and `/tmp/dociler-rustup`. They are temporary,
outside the repository, and were not added to shell startup files. Use the normal
rustup setup described in `docs/development.md` on other machines.

## Decisions in force

- Product name and command: Dociler / `dociler`.
- Apache-2.0 license.
- Rust native CLI/TUI; Docker deferred.
- Managed `llama.cpp` local runtime.
- Qwen3.5 4B and 9B Q4_K_M primary candidates, qualification-gated.
- Word-processing formats only in v1.
- Read-only workspace by default; persistent per-workspace write grants plus
  per-write confirmation.
- Built-in skills only.
- Ephemeral sessions and document indexes.
- Authenticated LAN API; remote document transmission requires session consent.
- Start with two substantive crates and add service modules as features arrive.
  No third-party runtime crate dependencies in M1.
- Rust 1.85.1 is the reproducible initial toolchain (edition 2024, MSRV 1.85).
  Update it deliberately with dependency/toolchain changes.
- ADR-012: bounded versioned JSON, OS-local config/data paths, no startup writes,
  exact workspace grant identity, and no plaintext credential fallback.
- M2 direct dependency pins are documented in `docs/development.md`; transitives
  are locked. Remote profiles/schema migration are not introduced prematurely.
- Session cap: 1 MiB of text / 512 messages, reject overflow without evicting old
  messages. This is not the future inference tokenizer/context budget.

## Verification

### M2 — 2026-09-11

Local Rust commands use the same isolated toolchain environment as M1 below.

- **passed:** `cargo fmt --all -- --check`.
- **passed:** `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --all-targets --locked --offline`: 20 tests
  (9 executable/service tests and 11 core foundation tests).
- **passed:** `cargo test --workspace --all-targets --release --locked --offline`:
  the same 20 tests against optimized code, including isolated `config init`.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet.
- **passed:** `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`.
- **passed:** `cargo build --workspace --release --locked --offline`; release
  `doctor`, `config paths`, and `config show` smoke runs. No real-user config
  initialized; all initialization tests use isolated temporary directories.
- **passed:** `python3 scripts/check_docs.py`: 15 Markdown files, 19 relative
  links, title/heading/fence checks, and README coverage.
- **passed:** `git diff --check` and final working-tree review.
- **failed:** initial sandboxed `cargo check --workspace` dependency download
  due to blocked DNS; **passed:** authorized network retry, followed by offline
  checks. Only Rust crate dependencies downloaded, no models/inference runtime.
- **not run:** hosted CI, macOS/Windows/Linux ARM64 execution, Windows ACL/native
  keychain validation, vulnerability/license audits, and all later feature/model/
  installer/release qualification gates. Local results do not complete those gates.

### M1 — 2026-09-11

Local Rust commands used `PATH=/tmp/dociler-cargo/bin:$PATH`,
`CARGO_HOME=/tmp/dociler-cargo`, and `RUSTUP_HOME=/tmp/dociler-rustup`.

- **passed:** `cargo fmt --all -- --check`.
- **passed:** `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --all-targets --locked --offline`: six tests.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet.
- **passed:** `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`.
- **passed:** `cargo build --workspace --release --locked --offline` and native
  Linux x86-64 release smoke runs for bare launch, `--version`, and `doctor`.
- **passed:** `python3 scripts/check_docs.py`: 14 Markdown files, 15 relative
  links, title/heading/fence checks, and README coverage.
- **passed:** workflow YAML parsed with PyYAML BaseLoader; asserted expected
  events, read-only permissions, and five native runner entries. This is static
  validation, not execution of GitHub Actions.
- **passed:** `git diff --check` and `git check-ignore` for build/model/state paths.
- **not run:** GitHub-hosted native jobs; macOS, Windows, and Linux ARM64 binaries.
- **not run:** future feature, minimum-OS, constrained-memory, model-quality,
  installer, supply-chain audit, and release qualification gates. No models or
  inference runtimes were downloaded. Rust compiler tooling alone was installed.

### Historical M0 — 2026-09-09

These results describe the documentation-only reset, before code was added.

- **Passed — archive target:** `git rev-list -n 1 archive/legacy-ollama`
  resolved to `c0f9c1f5236820fd5d9c717c80e7f44838a3d195`.
- **Passed — documentation-only tree:** excluding Git and environment-managed
  `.agents`/`.codex` directories, the working filesystem contains only five root
  handoff/license files and nine files under `/docs`; no Python, shell, Rust,
  TOML, Docker, Compose, Modelfile, Ollama, FastAPI, gateway, skills, setup,
  health-check, or GitLab CI implementation remains.
- **Passed — Markdown structure and links:** a read-only validation checked all
  13 Markdown files, required-document presence, exactly one H1 per file,
  non-skipping heading order, all 13 relative links, and every README document
  reference.
- **Passed — patch hygiene:** `git diff --check` returned no errors.
- **Passed — requirement coverage:** the approved decisions are represented in
  the implementation plan and routed to dedicated product, architecture, CLI,
  API, pipeline, runtime, privacy, testing, and decision documents.
- **Not run — application tests:** no application implementation or application
  test suite exists by design at milestone M0.

## Activity log

Do not delete or reorder entries. Append corrections and future progress.

| UTC timestamp | Agent | Task | Changes | Files | Verification | Outcome | Next handoff |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2026-09-09T03:03:43Z | Codex `/root` | Documentation-only reset | Tagged legacy state, removed implementation, and created the Dociler handoff package | Repository-wide documentation reset | Tag, tree, Markdown, coverage, and diff checks passed; application tests not run because none exist | COMPLETE | Await explicit authorization before scaffolding Rust or CI |
| 2026-09-11T16:32:10Z | Codex `/root` | Begin M1 after user authorization | Prepare Rust workspace, CLI entrypoint, checks, and CI; starting tree clean | Cargo manifests, crates, workflows, development docs, README, checkpoint | Verification pending; Rust absent from PATH | IN PROGRESS | Build and verify the foundation |
| 2026-09-11T16:40:57Z | Codex `/root` | M1 local implementation and verification | Added runnable CLI/core, six tests, toolchain/lockfile, CI, documentation checker, ignores, and development handoff; resolves prior in-progress entry | Cargo manifests/lock, rust-toolchain.toml, crates/, .github/, .gitignore, scripts/check_docs.py, README, implementation plan, development/testing docs, checkpoint | passed: fmt, Clippy, six tests, docs, release build/smoke, document links, static YAML, ignore and diff checks; not run: hosted CI and later feature/release gates | LOCAL WORK COMPLETE; HOSTED VALIDATION PENDING | Run hosted CI and begin M2 configuration/session foundation |
| 2026-09-11T16:45:07Z | Codex `/root` | Begin authorized M2 continuation | Add settings/path/workspace/session services, credential interface, CLI config inspection/init, tests and handoff; clean starting tree | Cargo manifests/lock, crates/, README, docs/, CHECKPOINT.md | not run: M2 checks pending | IN PROGRESS | Implement and verify M2 without model downloads or document persistence |
| 2026-09-11T16:54:06Z | Codex `/root` | M2 local implementation and verification | Added strict private config/init, OS paths, canonical grant policy, memory-only sessions, credential boundary, CLI commands, 14 new tests, schema docs and ADR-012; resolves prior in-progress entry | Cargo.toml/lock, core manifest/modules/tests, CLI/tests, README, docs/configuration.md, architecture/development/testing/decisions docs, CHECKPOINT.md | passed: fmt, Clippy, 20 debug and 20 release test executions, Rust docs, release build/smoke, Markdown and diff; failed: initial sandbox dependency DNS, resolved by authorized retry; not run: hosted/platform/keychain/audit/later release gates | LOCAL WORK COMPLETE; HOSTED VALIDATION PENDING | Run hosted CI; begin M3 remote profile validation and credential adapters before onboarding/streaming chat |
| 2026-09-11T16:58:57Z | Codex `/root` | Begin authorized M3 continuation | Add remote profiles, secure endpoint resolution, OS keychain adapter, upstream verification, one-shot streaming chat, mock-server tests and handoff; clean starting tree | Cargo manifests/lock, crates/, README, docs/, CHECKPOINT.md | not run: M3 checks pending | IN PROGRESS | Implement and verify remote text integration without documents or local models |
