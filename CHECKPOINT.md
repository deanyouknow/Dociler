# Project Checkpoint

Last updated: 2026-09-20T13:59:08Z

## Current status

**Status:** M3 remote text flow plus M4 read-only hardware/preflight and pinned
asset/cache-verification foundations are locally implemented and verified;
native cross-platform validation, downloads, and local runtime execution remain
pending.

**Active milestone:** M4 — Hardware admission, verified local asset management,
and runtime integration (read-only inventory/preflight plus pinned manifests and
cache verification complete locally; downloads, extraction, runtime probe, and
inference pending).

The user authorized implementation on 2026-09-11. The workspace now builds a
`dociler` executable with help, version, workspace/platform diagnostics, config
inspection, and explicit default-settings initialization. It includes canonical
workspace policy lookup, bounded memory-only session types, and a credential
interface. It now validates and stores remote profiles, uses native credential
services for optional keys, verifies OpenAI-compatible model listing and minimal
generation, and streams one text response from stdin with memory-only history.
It now also opens an initial interactive terminal UI on a real TTY, maintains a
bounded multi-turn session in memory, renders streamed output without blocking
ordinary input, exposes core slash commands, and aborts active HTTP requests on
cancellation. A profile-free launch and `/connect add` now provide verified
remote setup with masked optional key entry and native-only credential storage.
Profiles can now be removed after explicit confirmation, and keys can be
verified, rotated, or cleared without entering configuration or command-line
arguments. Saved profiles can be refreshed, re-checked, and edited with
verification, concurrent-target conflict detection, visible connection state,
and explicit consent before an existing key is sent to a changed origin. It has
an immutable model/runtime manifest and can list or SHA-256-verify existing
cache files without mutation. It has no local-backend onboarding, downloads,
runtime extraction/execution, document parsing, settings/grant update UI, or
Dociler API yet. The five-platform CI workflow is defined; its hosted result
could not be retrieved from this environment.

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
- Added strict remote profiles and `connect verify`, `connect add`, `connect list`,
  and `run NAME` commands with a stdin-only, streamed text path.
- Integrated Reqwest/rustls with proxy inheritance and redirects disabled,
  per-client DNS pinning, HTTPS/public and private-address HTTP policy, bounded
  OpenAI JSON/SSE parsing, exact model discovery, and minimal-generation checks.
- Integrated macOS Keychain, Windows Credential Manager, and Linux Secret Service
  through a keyring adapter with no plaintext fallback; the Linux transport is
  pure Rust and does not add a system `libdbus` build prerequisite.
- Added eight M3 tests (28 total) covering endpoint normalization/policy,
  redirect and model rejection, limits, bearer auth, Unicode SSE, profile
  persistence, and executable add/list/run behavior with loopback mock servers.
- Documented the first remote workflow, protocol/transport limits, dependency
  boundary, and ADR-013.
- Added a native Ratatui/Crossterm alternate-screen terminal surface for bare
  interactive launch and `chat [PROFILE]`, while non-terminal bare launch remains
  plain help and the existing one-shot commands remain stable.
- Added memory-only multi-turn remote chat, streaming partial output, background
  generation, Escape/Ctrl+C cancellation, bounded Unicode input and
  paste, terminal-state restoration, scrolling, and continuously visible
  workspace/profile/read-only/memory-only/API-off state.
- Added working `/help`, `/status`, saved-profile `/connect`, confirmed `/clear`,
  and `/exit`; unimplemented planned commands fail visibly. Added five terminal
  state/render tests, bringing the current Linux suite to 33 tests.
- Documented the implemented terminal boundary, operational commands,
  dependency pins, remaining UX gaps, and ADR-014.
- Extracted a provider-independent generation controller that owns bounded
  session commits, zeroizing fragment delivery, worker failure containment, and
  cancellation; deterministic fake backends cover successful and cancelled turns.
- Added worker-local cancellable async Reqwest transport for model discovery,
  minimal verification generation, and streamed chat. Cancellation now drops a
  stalled HTTP response instead of waiting for the 120-second request timeout.
- Added verified in-TUI remote onboarding on first profile-free launch and
  `/connect add`: validated name/URL/model steps, optional masked API key, no
  save before both verification checks, native credential storage, and rollback
  on configuration failure or pre-commit cancellation.
- Centralized CLI and TUI profile installation in a tested core service; added
  seven tests across controller, onboarding/profile transaction, async SSE, and
  stalled-transport cancellation, bringing the Linux suite to 40 tests.
- Added shared profile removal and credential-rotation services with verified
  candidate authentication, cancellation checks, native-key rollback, explicit
  orphan-cleanup warnings, and no plaintext fallback.
- Added scriptable `connect remove NAME --confirm`, `connect key NAME`, and
  `connect key-clear NAME --confirm`, with specific missing-confirmation/key
  diagnostics and executable integration coverage.
- Added TUI `/connect remove NAME` exact-repeat confirmation, `/connect key NAME`
  masked key/keyless flow, active-context reset, and setup `/back`/`/cancel`
  controls. The Linux suite is now 48 tests.
- Documented profile transaction ordering and ADR-015.
- Added a real Linux pseudo-terminal harness backed by an ephemeral loopback
  provider. It covers first-run profile verification, contextual two-turn
  streaming, stalled-response cancellation/disconnect, resize survival,
  alternate-screen restoration, clean exit, and absence of conversation data in
  saved configuration. Linux native CI jobs run it against the release binary.
- Added `connect check NAME` and verified `connect edit NAME URL MODEL` commands,
  plus TUI `/connect refresh`, `/connect check NAME`, and prefilled `/connect edit
  NAME` flows with visible unchecked/checking/ready/attention state.
- Added destination-bound credential protection: authenticated cross-origin
  edits stop before key retrieval/network access until a CLI flag or typed TUI
  confirmation explicitly approves the normalized destination.
- Reloaded settings after long profile verification, preserved unrelated
  concurrent changes, rejected stale target edits/rotations, refreshed TUI state
  before commands/prompts, and blocked prompt transmission after an active
profile changed. Added ADR-016 and tests, bringing the Linux suite to 56.
- Added a process-enumeration-free hardware service with host/lower-cgroup RAM,
  available memory, CPU counts/features, conservative thread planning, future
  asset-filesystem capacity, and unverified Metal candidacy on macOS.
- Added read-only `model status` plus hardware/preflight output in `doctor`.
  Lite/Pro results distinguish blocked, incomplete, experimental, and ready for
  a required runtime probe without creating directories or downloading assets.
- Added decimal 6/8/16 GB class handling, 5.5/11.5 GiB working-set floors,
  provisional model-plus-reserve disk floors, pinned minimal-feature sysinfo
  0.33.1, ADR-017, and eight tests, bringing the Linux suite to 64.
- Added `dociler-assets-v1` with immutable revisions and exact byte/SHA-256
  metadata for both GGUF profiles and five official llama.cpp `b10809` CPU/Metal
  target archives at the full v0.4.0 commit.
- Added read-only `model list` and `model verify [PROFILE]`, versioned OS-data
  paths, fail-closed file/type/size/hash inspection, absolute data/cache test
  overrides, pinned SHA-2 0.10.9, ADR-018, and seven tests, bringing the Linux
  suite to 71. No model or runtime archive was downloaded or executed.

## Work in progress

No local code unit is currently in progress. M1–M4 hosted/native execution
remains unverified outside this Linux x86-64 environment.

## Next recommended task

Continue M4 with a consent-gated, cancellable/resumable download service that
stages partial files under the versioned asset directory, enforces the built-in
URL/size/SHA-256, and publishes atomically without overwriting a valid cache.
Keep archive extraction, runtime execution/probing, removal/repair UI, and TUI
local selection as later bounded units. Validate M3/M4 on hosted target
platforms when access is available. Implementation remains authorized.

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
- ADR-013: remote URLs are normalized to `/v1`, DNS-pinned per client, proxy-free,
  redirect-free, HTTPS for public addresses, and HTTP only for all-private or
  loopback resolutions; credentials use native stores only.
- Remote profile settings persist only name/base URL/upstream model/a
  credential-presence marker, never the secret. Removal requires explicit
  confirmation; credential-policy changes require upstream verification.
- One-shot remote prompts are UTF-8 stdin only (64 KiB maximum); streamed answer
  text is capped at 1 MiB and remains memory-only unless the caller redirects it.
- ADR-014: the M3 terminal uses pinned Ratatui 0.29/Crossterm 0.28 on the main
  event/render thread and an in-process worker/channel for generation.
  Bare non-terminal use remains plain help; prompt, transcript, and partial
  output remain memory-only with best-effort zeroization.
- Remote generation and profile verification use a worker-local current-thread
  Tokio runtime with 20 ms cancellation observation; dropping the request aborts
  peer-stalled HTTP. OS DNS resolution and credential-service calls remain
  blocking platform boundaries.
- Profile installation re-loads current validated settings, verifies before
  mutation, stores any key only in the native credential service, atomically
  saves non-secret settings, and best-effort deletes a newly stored key if save
  or pre-commit cancellation fails.
- ADR-015: removal/key clearing commits non-secret configuration before native
  key cleanup and reports orphan warnings; key additions verify first and roll
  back the native write if marker publication fails.
- ADR-016: refresh profile state before disclosure, reset context and require
  prompt resubmission after an active-profile change, compare verified target
  state before mutation, and require explicit consent before reusing a stored
  key at a different origin.
- ADR-017: hardware inventory is local/read-only/ephemeral and process-free;
  decimal RAM classes, binary working-set floors, lower cgroup limits, and
  fail-closed disk/memory checks only permit a later runtime probe, never final
  admission or automatic accelerator selection.
- ADR-018: compile immutable model revisions and the exact llama.cpp
  release/build/commit plus per-target archive metadata into the executable;
  keep metadata listing separate from explicit hashing, reject unsafe cache
  components, and enable no download/extraction/execution as a side effect.

## Verification

### M4 pinned asset manifest and read-only cache verification — 2026-09-20

Local checks used isolated Rust 1.85.1 tooling. Upstream validation queried only
official Hugging Face file pointers/pages and the official GitHub release API.
No GGUF or runtime archive was downloaded, extracted, or executed.

- **passed:** exact model filenames, immutable repository revisions, byte sizes,
  and SHA-256 values against the two official Hugging Face pointers; llama.cpp
  v0.4.0 to `b10809`/full-commit mapping and five platform archive URLs, byte
  sizes, and SHA-256 digests against official GitHub release metadata.
- **passed:** `cargo fmt --all -- --check` and `cargo clippy --workspace
  --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --locked --offline`: 71 tests (13 terminal,
  13 CLI, 29 core unit, 11 foundation, 5 mock remote integration).
- **passed:** `cargo test --workspace --release --locked --offline`: the same 71
  tests against optimized code; doctests (none defined) and release build.
- **passed:** manifest uniqueness/immutability/platform mapping, synthetic exact
  size/SHA-256 success and mismatch cases, missing-cache no-write behavior, Unix
  symlink rejection, absolute app-directory overrides, and executable list/
  verify read-only behavior with the expected nonzero missing-asset result.
- **passed:** release `model list`/`model verify dociler-lite` smoke using an
  isolated empty directory, with `find` confirming no file/directory creation;
  Rust docs with warnings denied; release PTY regression; Markdown links/headings
  (`16` files, `27` relative links); and `git diff --check`.
- **failed then resolved:** one CLI assertion expected the wrong display prefix;
  strict Clippy then rejected an eight-argument manifest helper. The assertion
  and helper shape were corrected before both full debug/release reruns passed.
- **not run:** hashing real multi-gigabyte cached artifacts, downloading/resuming
  assets, archive extraction/content inventory, runtime load/generation probes,
  GPU packages beyond the pinned macOS archives, constrained-host model quality,
  hosted CI, or native macOS/Windows/Linux ARM64 execution.
- **not run:** documents, local inference, LAN API, installers, dependency/
  license audits, signing/provenance, and later release gates.

### M4 hardware inventory and admission foundation — 2026-09-20

Local checks used isolated Rust 1.85.1 tooling. Hardware inspection made no
network request and created no model/runtime directory. Mock-provider tests used
only ephemeral `127.0.0.1` ports.

- **passed:** `cargo fmt --all -- --check` and `cargo clippy --workspace
  --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --locked --offline`: 64 tests (13 terminal,
  12 CLI, 23 core unit, 11 foundation, 5 mock remote integration).
- **passed:** `cargo test --workspace --release --locked --offline`: the same 64
  tests against optimized code.
- **passed:** deterministic below-6-GB rejection, 6–8-GB experimental Lite,
  decimal 8/16 GB boundary, available-memory/disk failure, ready-for-probe, and
  fail-closed incomplete-inventory cases; live read-only inventory and CLI
  no-workspace-write checks.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet;
  `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`;
  `cargo build --workspace --release --locked --offline`.
- **passed:** release `model status`/`doctor` smoke and release PTY regression;
  `python3 scripts/check_docs.py` (16 Markdown files, 27 relative links); and
  `git diff --check`.
- **failed then resolved:** temporary Rust tooling had expired and was restored
  under `/tmp`; an initial GiB interpretation would have rejected ordinary
  marketed 8/16 GB hosts, so class thresholds were corrected to decimal GB and
  protected with boundary tests before final verification.
- **not run:** native macOS/Windows/ARM64 hardware results, CUDA/Vulkan discovery,
  packaged runtime probes, model/runtime cache or downloads, constrained-host
  memory qualification, and hosted CI.
- **not run:** documents, local inference, LAN API, installers, audits, supply
  chain, and later release gates.

### M3 profile refresh, edit, and reconnect integration — 2026-09-16

Local checks used isolated Rust 1.85.1 tooling. Mock providers bound only
ephemeral `127.0.0.1` ports and made no public-provider requests.

- **passed:** `cargo fmt --all -- --check` and `cargo clippy --workspace
  --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --locked --offline`: 56 tests (13 terminal,
  11 CLI, 16 core unit, 11 foundation, 5 mock remote integration).
- **passed:** `cargo test --workspace --release --locked --offline`: the same 56
  tests against optimized code.
- **passed:** concurrent unrelated-profile merge, stale-target conflict,
  credential preservation, pre-key-access cross-origin consent, TUI refresh and
  context reset, blocked prompt disclosure, status recovery action, and
  executable add/check/edit/remove lifecycle coverage.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet;
  `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`;
  `cargo build --workspace --release --locked --offline`.
- **passed:** debug and release `scripts/test_tui_pty.py` runs, including saved
  profile health checking; release help/doctor smoke; Python syntax parse;
  `python3 scripts/check_docs.py` (16 Markdown files, 27 relative links); and
  `git diff --check`.
- **not run:** real Keychain/Credential Manager/Secret Service mutation, hosted
  macOS/Windows/Linux ARM64 CI, live providers, and full adversarial
  multi-process stress/locking tests.
- **not run:** local models, documents, LAN API, constrained-memory/model
  qualification, installers, audits, supply chain, and later release gates.

### M3 mock-provider pseudo-terminal integration — 2026-09-16

- **passed:** `python3 scripts/test_tui_pty.py target/debug/dociler` and the same
  harness against `target/release/dociler` on Linux x86-64.
- **passed:** real-PTY first-run onboarding against an ephemeral loopback mock;
  the verified non-secret profile was saved to an isolated temporary config.
- **passed:** two streamed turns with the first user/assistant exchange present
  in the second upstream request; generated/user content was absent from config.
- **passed:** Escape dropped a deliberately stalled SSE connection; the TUI
  remained usable, survived a `SIGWINCH` resize, exited 0, and emitted both
  alternate-screen enter and leave sequences.
- **passed:** Python syntax parse, static CI YAML assertion for Linux-only release
  PTY execution, `python3 scripts/check_docs.py`, and `git diff --check`.
- **failed then resolved:** raw Ratatui output is an incremental cursor-addressed
  stream, not a screen snapshot. Brittle text-title waits were replaced with
  initial-screen confirmation plus mock request events, config publication, and
  protocol/exit assertions; debug and release reruns then passed.
- **not run:** macOS/Windows terminal behavior, hosted Linux ARM64 execution,
  live providers, and real native credential services.

### M3 profile lifecycle and onboarding retry — 2026-09-16

Local checks used isolated Rust 1.85.1 tooling. Mock providers bound only
ephemeral `127.0.0.1` ports and made no public-provider requests.

- **passed:** `cargo fmt --all -- --check`.
- **passed:** `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --all-targets --locked --offline`: 48 tests
  (9 terminal, 11 CLI, 12 core unit, 11 foundation, 5 mock remote integration).
- **passed:** `cargo test --workspace --all-targets --release --locked --offline`:
  the same 48 tests against optimized code.
- **passed:** core removal, orphan-warning, key-marker add/clear, pre-cancelled
  mutation, and cancellation-during-native-write rollback tests; TUI
  back/cancel, masked rotation, exact-repeat removal, and context-reset tests;
  executable add/list/chat/keyless/remove transaction coverage.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet;
  `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`.
- **passed:** `cargo build --workspace --release --locked --offline`; isolated
  release version, doctor, and help smoke runs.
- **passed:** `python3 scripts/check_docs.py`: 16 Markdown files, 27 relative
  links, title/heading/fence checks, and README coverage; `git diff --check`.
- **failed then resolved:** the workspace restart removed temporary Rust tools;
  official rustup restored pinned 1.85.1 in `/tmp`, and the locked crate set was
  downloaded there before all final Cargo checks ran offline. No model/runtime
  assets were downloaded.
- **not run:** full mock-backed pseudo-terminal chat/cancel/resize; live provider
  calls; real Keychain/Credential Manager/Secret Service mutation; hosted
  macOS/Windows/Linux ARM64 CI; concurrent-process refresh and endpoint/model
  editing.
- **not run:** local models, documents, LAN API, constrained-memory/model
  qualification, installers, audits, supply chain, and later release gates.

### M3 controller, cancellable transport, and remote onboarding — 2026-09-14

Local checks used the isolated Rust 1.85.1 environment. Mock providers bind only
ephemeral `127.0.0.1` ports and make no public-provider requests.

- **passed:** `cargo fmt --all -- --check`.
- **passed:** `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --all-targets --locked --offline`: 40 tests
  (6 terminal, 11 CLI, 7 core unit, 11 foundation, 5 mock remote integration).
- **passed:** `cargo test --workspace --all-targets --release --locked --offline`:
  the same 40 tests against optimized code.
- **passed:** deterministic fake-backend completion/cancellation, profile and
  credential separation, cancellation-before/during-verification with no writes,
  cancellable Unicode SSE, and peer-stalled transport abort in under one second.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet;
  `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`.
- **passed:** `cargo build --workspace --release --locked --offline`; release
  doctor and redirected bare-launch smoke runs.
- **passed:** pseudo-terminal traversal through profile name, URL, model, and
  hidden-key step followed by `/exit`; exit code 0 and no configuration created.
- **passed:** `python3 scripts/check_docs.py`: 16 Markdown files, 27 relative
  links, title/heading/fence checks, and README coverage; `git diff --check`.
- **failed then resolved:** the first offline check lacked the newly enabled
  Reqwest stream dependency; an authorized Cargo check resolved the locked
  `tokio-util` and WASM-target entries and downloaded `tokio-util`, after which
  all work ran offline.
- **failed then resolved:** the first stalled-transport test exposed a Tokio
  runtime with time but not I/O enabled; enabling all runtime drivers made the
  focused and full cancellation suites pass.
- **not run:** successful TUI onboarding against a live provider/native keychain;
  live-provider chat/cancellation; blocking DNS or credential-call interruption;
  full mock-backed pseudo-terminal conversation; resize; macOS/Windows/Linux
  ARM64 and hosted CI.
- **not run:** profile removal/rotation, local models, documents, LAN API,
  constrained-memory/model qualification, installers, audits, supply chain, and
  later release gates. No models or inference runtimes were downloaded.

### M3 initial interactive terminal — 2026-09-14

The same isolated Rust 1.85.1 environment was used. Loopback provider tests bind
only ephemeral `127.0.0.1` ports and make no public-provider requests.

- **passed:** `cargo fmt --all -- --check`.
- **passed:** `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --all-targets --locked --offline`: 33 tests
  (5 terminal unit, 11 CLI, 2 core remote unit, 11 foundation, 4 mock remote).
- **passed:** `cargo test --workspace --all-targets --release --locked --offline`:
  the same 33 tests against optimized code.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet.
- **passed:** `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`.
- **passed:** `cargo build --workspace --release --locked --offline`; release
  help, doctor, profile-list, and redirected bare-launch smoke runs.
- **passed:** delayed-input pseudo-terminal smoke launch and `/exit` using
  `script`; it exited 0 after exercising alternate-screen setup/cleanup.
- **passed:** `python3 scripts/check_docs.py`: 16 Markdown files, 27 relative
  links, title/heading/fence checks, and README coverage; `git diff --check`.
- **failed then resolved:** the first off-screen render assertion exposed a
  clipped header; increasing its layout height made state visible and the full
  suite passed.
- **failed then resolved:** the first pseudo-terminal smoke injected `/exit`
  before raw event setup and timed out; a one-second delayed injection exercised
  the intended event path and exited successfully.
- **not run:** live provider chat; live credential-store interaction; a real
  person/manual terminal review; provider-independent fake-backend transcript
  flow; stalled-transport abort; terminal resize; macOS/Windows/Linux ARM64 and
  hosted CI.
- **not run:** onboarding, documents, local model/runtime, LAN API, memory/model
  qualification, installers, audit, supply-chain, and later release gates. No
  models or inference runtimes were downloaded.

### M3 remote foundation — 2026-09-11 to 2026-09-14

The final checks used the isolated Rust 1.85.1 environment described below.
Loopback integration tests required permission to bind ephemeral `127.0.0.1`
ports; they made no public-provider requests.

- **passed:** `cargo fmt --all -- --check`.
- **passed:** `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- **passed:** `cargo test --workspace --all-targets --locked --offline`: 28 tests
  (11 CLI, 2 core unit, 11 foundation, 4 mock remote integration).
- **passed:** already-built optimized test binaries for the same 28 tests after
  the earlier release build completed; release `doctor` and `connect list` smoke
  runs also passed.
- **passed:** `cargo test --workspace --doc --locked --offline`: no doctests yet.
- **passed:** `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked --offline`.
- **passed:** `python3 scripts/check_docs.py`: 16 Markdown files, 27 relative
  links, title/heading/fence checks, and README coverage.
- **passed:** `git diff --check`; M3 implementation is present in commit
  `68549eb4e82e9e3070e92c72259a3231bb024ee6` before this checkpoint update.
- **failed then resolved:** the first Linux keyring feature attempted to compile
  system `libdbus`; it was replaced by the pure-Rust async Secret Service feature,
  after which build, lint, and tests passed without that system package.
- **not run:** live third-party provider calls; actual macOS/Windows/Linux Secret
  Service credential writes; interactive TUI behavior; hosted macOS, Windows,
  and Linux ARM64 CI. GitHub Actions status was unavailable because the configured
  token is invalid and the unauthenticated repository API returned 404.
- **not run:** document consent/ingestion, local model/runtime, memory/model
  quality, installer, audit, supply-chain, and later release gates. No models or
  inference runtimes were downloaded.

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
| 2026-09-14T00:22:07Z | Codex `/root` | Complete M3 remote foundation and resume interrupted verification | Confirmed committed remote profile/keychain/HTTP/SSE integration, reran current-tree debug and optimized tests, docs/lints, release smoke, and recorded limitations; resolves prior in-progress entry | Remote/config/credential/session core, CLI commands/tests, Cargo manifests/lock, README, remote/config/architecture/CLI/security/development/testing/decision docs, CHECKPOINT.md | passed: fmt, Clippy, 28 debug tests, 28 optimized test binaries, Rust docs, release smoke, 16-page Markdown check, diff; failed then resolved: system-libdbus keyring feature; not run: hosted platforms/live providers/live keychains/TUI/later gates | REMOTE FOUNDATION COMPLETE; M3 INTERACTIVE UX PENDING | Build testable multi-turn controller and interactive terminal onboarding; review hosted CI with valid access |
| 2026-09-14T00:27:39Z | Codex `/root` | Continue authorized M3 interactive integration | Add chat controller, terminal surface, core slash commands, visible state, deterministic fake-backend tests, docs and handoff; existing checkpoint-only change is prior agent work | Cargo manifests/lock, core chat/session services, CLI terminal modules/tests, README, docs/, CHECKPOINT.md | not run: interactive M3 checks pending | IN PROGRESS | Implement terminal chat without documents, local models, LAN serving, or persistent transcripts |
| 2026-09-14T00:44:17Z | Codex `/root` | Complete initial M3 interactive terminal integration | Added real-TTY launch/chat routing, memory-only multi-turn state, background streaming, cooperative cancellation, core commands, terminal restoration/rendering, five TUI tests, dependency pins, docs and ADR-014; resolves prior in-progress entry, except fake-backend/controller extraction is handed forward explicitly | Cargo manifests/lock, `dociler` main/TUI/tests, core remote errors, README, architecture/CLI/development/remote/testing/decision docs, CHECKPOINT.md | passed: fmt, Clippy, 33 debug and release tests, Rust docs, release build/non-TTY smoke, delayed-input PTY exit, Markdown and diff checks; failed then resolved: clipped test header and early PTY input; not run: live/manual/platform/provider/fake-backend/stalled-abort/later gates | INITIAL TERMINAL FOUNDATION COMPLETE; M3 REMAINS ACTIVE | Extract abort-capable provider-independent controller, add deterministic fake backend and in-TUI onboarding/hidden credentials, then broaden PTY/platform checks |
| 2026-09-14T00:46:47Z | Codex `/root` | Harden cancellation handoff after final review | Added cancellation checks after credential lookup/client creation and before committing a completed answer, preventing a late cancel from entering model context; records follow-up to the 00:44 entry | `crates/dociler/src/tui.rs`, CHECKPOINT.md | passed: fmt, Clippy, 33 debug tests, 33 optimized tests, release rebuild/doctor/non-TTY help, final delayed-input PTY exit, Markdown and diff checks | COMPLETE | Transport-level abort and deterministic fake-backend coverage remain the next M3 hardening task |
| 2026-09-14T00:49:25Z | Codex `/root` | Continue authorized M3 controller/onboarding integration | Extract provider-independent generation control with deterministic cancellation tests, then add safe in-TUI remote onboarding and hidden credential input within existing policy; preserve all current uncommitted M3 work | Core/controller, terminal state/rendering, configuration/credential adapters, tests, README/docs, CHECKPOINT.md | not run: integration checks pending | IN PROGRESS | Complete this bounded M3 hardening/onboarding unit without documents, local models, LAN serving, or persisted transcripts |
| 2026-09-14T01:11:24Z | Codex `/root` | Complete M3 controller, cancellable transport, and remote onboarding unit | Added provider-independent generation/session controller, zeroizing events, cancellable async verification/chat transport, transactional profile installer shared by CLI/TUI, automatic and `/connect add` four-step onboarding with masked optional key, tests and docs; resolves 00:49 entry | Cargo manifests/lock, core cancellation/chat/profile/remote modules and tests, CLI/TUI/tests, README, architecture/CLI/development/remote/privacy/testing/decision docs, CHECKPOINT.md | passed: fmt, Clippy, 40 debug and optimized tests, doctests/docs, release build/smoke, onboarding PTY/no-write check, Markdown/diff; failed then resolved: missing stream dependency offline and Tokio I/O driver; not run: live keychains/providers/platform CI/full PTY conversation/profile lifecycle/later gates | COMPLETE; M3 REMAINS ACTIVE | Add safe profile remove/key rotation and onboarding edit/retry, then mock-backed PTY chat/cancel/resize and platform validation |
| 2026-09-16T00:17:36Z | Codex `/root` | Continue authorized M3 profile lifecycle integration | Add transactional profile removal and credential rotation across core/CLI/TUI, onboarding back/retry controls, deterministic tests, documentation, and handoff; preserve all existing uncommitted M3 work | Core config/profile services, CLI/TUI state and commands, tests, README/docs, CHECKPOINT.md | not run: lifecycle checks pending | IN PROGRESS | Complete this M3 lifecycle unit without documents, local models, LAN serving, or transcript persistence |
| 2026-09-16T00:36:49Z | Codex `/root` | Complete M3 profile lifecycle and onboarding retry unit | Added shared safe removal/key rotation, CLI confirmations, TUI exact-repeat removal and masked credential update, setup back/cancel/retry, rollback/orphan handling, tests, docs, and ADR-015; resolves 00:17 entry | Core config/profile/remote services, CLI/TUI/tests, README, architecture/CLI/configuration/development/remote/privacy/testing/decision docs, CHECKPOINT.md | passed: fmt, strict Clippy, 48 debug and optimized tests, doctests/docs, release build/smoke, Markdown and diff checks; failed then resolved: temporary toolchain lost on restart; not run: mock PTY chat/resize, live keychains/providers, hosted platforms, later gates | COMPLETE; M3 REMAINS ACTIVE | Add mock-backed pseudo-terminal onboarding/chat/cancel/resize and terminal cleanup; then concurrent refresh/reconnect/editing and platform validation |
| 2026-09-16T00:38:50Z | Codex `/root` | Continue M3 pseudo-terminal integration | Add a loopback mock-provider PTY harness covering onboarding, streamed multi-turn chat, stalled-request cancellation, resize, terminal restoration, and CI wiring | PTY test script, CI, testing/development docs, CHECKPOINT.md | not run: PTY integration pending | IN PROGRESS | Complete deterministic Linux PTY coverage without live providers, real credentials, documents, or model downloads |
| 2026-09-16T00:44:16Z | Codex `/root` | Complete M3 mock-provider pseudo-terminal integration | Added deterministic real-PTY onboarding, contextual chat, stalled-stream cancel/disconnect, resize and alternate-screen cleanup coverage plus Linux native CI execution; resolves 00:38 entry | `scripts/test_tui_pty.py`, CI workflow, CLI/development/testing docs, CHECKPOINT.md | passed: debug and release PTY runs, Python parse, static CI YAML, Markdown and diff checks; failed then resolved: cursor-delta text waits replaced by semantic provider/config synchronization; not run: hosted/non-Linux/live provider/keychain/later gates | COMPLETE; M3 REMAINS ACTIVE | Add concurrent profile refresh, reconnect status, verified endpoint/model editing, then native credential/platform validation |
| 2026-09-16T00:46:34Z | Codex `/root` | Continue M3 profile refresh/edit/reconnect integration | Add config-backed profile refresh after concurrent changes, verified endpoint/model editing with credential preservation, actionable health/reconnect status, CLI/TUI flows, tests, docs, and handoff | Core profile/config services, CLI/TUI/tests, README/docs, CHECKPOINT.md | not run: refresh/edit/reconnect checks pending | IN PROGRESS | Complete this bounded M3 unit without documents, local models, LAN serving, or persisted transcripts |
| 2026-09-16T01:13:48Z | Codex `/root` | Complete M3 profile refresh/edit/reconnect integration | Added config-backed refresh before commands/prompts, verified profile checks/edits, visible connection recovery, concurrent-change merge/conflict handling, cross-origin credential consent, non-disclosure guard, tests, docs, and ADR-016; resolves 00:46 entry | Core profile/config/remote services, CLI/TUI/tests, PTY harness, README, architecture/CLI/configuration/development/remote/privacy/testing/decision docs, CHECKPOINT.md | passed: fmt, strict Clippy, 56 debug and optimized tests, doctests/docs, release build/help/doctor, debug/release PTY, Markdown/Python/diff checks; not run: live providers/keychains, hosted/non-Linux platforms, later gates | COMPLETE; M3 LOCAL TEXT FLOW COMPLETE | Validate native credential/platform behavior when available; otherwise begin M4 read-only hardware inventory/admission without downloads |
| 2026-09-20T13:25:33Z | Codex `/root` | Begin M4 hardware inventory/admission foundation | Add read-only cross-platform RAM/CPU/disk inventory, explicit Lite/Pro preflight outcomes, doctor/model status output, deterministic tests, docs, and handoff; clean starting tree | Cargo manifests/lock, core hardware/model admission services, CLI/tests, README/docs, CHECKPOINT.md | not run: M4 foundation checks pending | IN PROGRESS | Complete without runtime/model downloads, accelerator selection, local inference, documents, or persistent hardware data |
| 2026-09-20T13:38:43Z | Codex `/root` | Complete M4 hardware inventory/admission foundation | Added process-free RAM/CPU/disk/cgroup inventory, conservative Lite/Pro preflight states, `model status`, enriched doctor output, dependency pin, tests, docs, and ADR-017; resolves 13:25 entry | Cargo manifests/lock, core hardware service, CLI/tests, README, architecture/CLI/development/runtime/privacy/testing/decision docs, CHECKPOINT.md | passed: fmt, strict Clippy, 64 debug and optimized tests, doctests/docs, release build/status/doctor, release PTY, Markdown/diff checks; failed then resolved: missing temporary toolchain and GB/GiB class mismatch; not run: native non-Linux hardware, accelerators/runtime/models/downloads/later gates | COMPLETE; M4 FOUNDATION ACTIVE | Add a pinned asset-manifest and read-only cache verification layer after resolving exact model/runtime artifact metadata; do not download or execute assets yet |
| 2026-09-20T13:44:18Z | Codex `/root` | Continue M4 pinned-asset/cache integration | Validate authoritative model/runtime pins, add a versioned built-in manifest and read-only cache list/verification commands, deterministic tests, docs, and handoff; clean starting tree | Cargo manifests/lock, core asset service, CLI/tests, README/docs, CHECKPOINT.md | upstream metadata validation passed; implementation checks not run | IN PROGRESS | Complete without downloading, extracting, deleting, or executing any model/runtime asset |
| 2026-09-20T13:59:08Z | Codex `/root` | Complete M4 pinned-asset/cache integration | Added immutable model/runtime pins, versioned paths, metadata/full-hash cache inspection, `model list`/`model verify`, app-data overrides, seven tests, docs, and ADR-018; resolves 13:44 entry | Cargo manifests/lock, core asset/path services, CLI/tests, implementation plan, README, architecture/CLI/configuration/development/runtime/privacy/testing/decision docs, CHECKPOINT.md | passed: upstream official metadata, fmt, strict Clippy, 71 debug/optimized tests, doctests/docs, release build/list/verify smoke, no-write find, release PTY, Markdown/diff; failed then resolved: display assertion and helper lint; not run: real assets/downloads/extraction/runtime/platform/later gates | COMPLETE; M4 ASSET VERIFICATION ACTIVE | Add consent-gated resumable staging/download and atomic publication; do not extract or execute assets yet |
