# Dociler — Agent Handoff

Snapshot: 2026-09-26. Implementation baseline: commit `9e0384b` on `main`,
matching `origin/main` when this handoff began. This file preserves conversation
context for a new agent; it does not replace the product specification or live
checkpoint. Refresh Git status and the checkpoint before making changes.

## Start here

Read [agent instructions](AGENTS.md), [README](README.md), the
[approved implementation plan](IMPLEMENTATION_PLAN.md), and
[live checkpoint](CHECKPOINT.md), then the relevant supporting documents.
The plan defines the product; the checkpoint defines execution state. If they
conflict, record a blocker instead of silently choosing one.

The latest user request is **export context for another AI agent**, not build
another feature. This handoff changes documentation only. Implementation was
previously authorized and can resume with the checkpoint's next task.

## Conversation context

- The abandoned application analyzed sensitive financial documents using
  Ollama and Qwen2.5 3B. The user wanted a fresh, universal document assistant.
- The original brainstorming name was “Dompiler,” but the subsequently approved
  reset and implementation plan use **Dociler**, with command `dociler`.
  Likewise, the initially discussed 4B/8B choices became approved Qwen3.5
  **4B/9B** profiles. Do not revert these decisions from the early conversation.
- The documentation-only reset is already complete. **Do not wipe the project
  again.** Legacy code is recoverable from annotated tag `archive/legacy-ollama`
  at `c0f9c1f5236820fd5d9c717c80e7f44838a3d195`.
- The user then authorized implementation and repeatedly requested continuation.
  They also asked for verified changes to be committed and pushed; recent
  implementation units were published to `origin/main`. Inspect the current
  diff, branch, and authorization before publishing later work; never stage
  unrelated user changes.
- Most recently the user asked whether the project was near completion. The
  answer was **no: the foundations exist, but document analysis and sustained
  local chat are still missing**. Do not present diagnostic success as a
  finished assistant or an admitted low-memory model.
- An end-to-end document-reading alpha was suggested as a useful future
  delivery target. That suggestion was **not an approved reprioritization**.
  The canonical next task remains M4 safety/admission work.
- The user is now switching agents and wants to avoid reconstructing this
  conversation. Keep the checkpoint current after each meaningful work unit.

## User priorities and approved boundaries

The intended experience is a welcoming Codex/Claude-Code-like terminal: install
the harness, choose a remote OpenAI-compatible endpoint or a local model tier,
explicitly download local assets if needed, then chat about the workspace.
Optional API serving should let other harnesses use the managed model.

- Privacy and low memory matter: local-first, read-only by default, no telemetry,
  and ephemeral chats, extracted text, and indexes. Remote document disclosure
  requires explicit session consent.
- Native Rust with managed `llama.cpp`, not an Ollama dependency or Docker-first
  installation. Installers must not automatically download models.
- Target Lite: Qwen3.5 4B Q4_K_M, 8K context, 8 GB-class hosts. Target Pro:
  Qwen3.5 9B Q4_K_M, 16K context, 16 GB-class hosts. These are qualification-gated
  targets, not claims of present support on every device below those RAM sizes.
- Built-in document skills must apply consistently to CLI and gateway requests.
  Public aliases are configured profiles, **not newly trained models**; retain
  original attribution and licensing. Skills and the gateway are not built yet.
- V1 inputs: `.md`, `.pdf`, `.docx`, `.doc`, `.rtf`, `.odt`, `.txt`. No scanned-PDF
  OCR. Spreadsheets, presentations, web ingestion, Google Docs/MCP, arbitrary
  shell execution, and third-party skills remain deferred.
- Future writing requires a workspace grant and confirmation of each write;
  only Markdown/text support in-place editing. Other supported outputs are
  explicit exports, with legacy DOC remaining input-only.

See [requirements](docs/product-requirements.md),
[security](docs/security-and-privacy.md), and
[model/runtime qualification](docs/models-and-runtime.md) for precise limits.
Use the pinned manifest and that document for exact artifacts/checksums rather
than copying stale model URLs from conversation history.

## What actually works

M4 is active, not complete. Earlier work provides the Rust workspace, settings,
workspace policies, ephemeral sessions, native credential adapters, remote
provider verification, and streamed one-shot/multi-turn text chat. Bare
interactive `dociler` opens a terminal UI with remote-profile onboarding and
profile lifecycle controls; it does not yet read documents.

Local foundations include conservative hardware preflight, an immutable asset
manifest, full cache hashing, consent-gated resumable downloads, safe runtime
extraction/inventory, and private authenticated loopback runtime probes.

The model-load probe is **diagnostic only**: CPU, 1,024-token context, one slot,
and at most 16 generated tokens, followed by shutdown. Signal cancellation and
Unix sidecar process-group separation exist. Linux memory reporting samples
the server child's `VmHWM`; it is **not full process-group accounting or an
enforced memory limit**. A tiny real GGUF has passed protocol testing; actual
Lite/Pro qualification has not.

## What is still missing

- M4: full-session/process-group memory policy and enforcement, full-context
  Lite/Pro memory and quality evidence, native platform checks, compiled runtime
  per-file/CPU-instruction release gates, persistent local chat, local onboarding,
  and model removal/repair UX.
- M5: document discovery, isolated Extractous parsing, canonical AST, chunking,
  retrieval/token budgets, long-document analysis, citations, and built-in skills.
- M6: controlled edits/exports, authenticated Dociler API, remote-document
  consent, and background-server lifecycle.
- M7: installers/updater/release packaging, security/license audits, provenance,
  and full hardware/platform/release qualification.

These are substantial product features, not just final cleanup. The defined
five-platform CI workflow is not evidence that hosted or native-platform tests
have been observed passing. See [milestones](docs/development.md),
[document pipeline](docs/document-pipeline.md), and [API contract](docs/api.md).

## Next bounded work

Follow the checkpoint: design and test full process-group peak-memory accounting
and an enforceable constrained-host admission/abort policy, including Dociler,
runtime, and eventual document work. Verify cancellation during a real lengthy
Lite/Pro load and make multi-GB rehashing cancellable. Then attempt pinned profile
diagnostics and native platform checks. Keep local chat disabled until required
full-context memory and model-quality gates pass.

A bounded slice within that task is cancellable model/runtime hash loops with
deterministic tests preserving integrity checks and publication guarantees.
Do not confuse a diagnostic memory envelope with final document-workflow
qualification: parser/UI workloads and cross-platform enforcement still need
their own evidence. Any proposed change in delivery order should be explicit
and recorded rather than silently declaring M4 finished.

## Code and documentation map

| Area | Entry points |
| --- | --- |
| CLI, terminal, probe signals | [main.rs](crates/dociler/src/main.rs), [tui.rs](crates/dociler/src/tui.rs), [cli_signals.rs](crates/dociler/src/cli_signals.rs) |
| Remote transport and session orchestration | [remote.rs](crates/dociler-core/src/remote.rs), [profiles.rs](crates/dociler-core/src/profiles.rs), [chat.rs](crates/dociler-core/src/chat.rs) |
| Settings, secrets, workspace access | [config.rs](crates/dociler-core/src/config.rs), [credentials.rs](crates/dociler-core/src/credentials.rs), [workspace.rs](crates/dociler-core/src/workspace.rs) |
| Pinned assets and verified installation | [assets.rs](crates/dociler-core/src/assets.rs), [downloads.rs](crates/dociler-core/src/downloads.rs), [runtime_install.rs](crates/dociler-core/src/runtime_install.rs) |
| Active M4 safety work | [hardware.rs](crates/dociler-core/src/hardware.rs), [runtime_probe.rs](crates/dociler-core/src/runtime_probe.rs), [model_probe.rs](crates/dociler-core/src/model_probe.rs), [probe_memory.rs](crates/dociler-core/src/probe_memory.rs), [cancellation.rs](crates/dociler-core/src/cancellation.rs) |
| Verification and CI | [CLI tests](crates/dociler/tests/cli.rs), [remote tests](crates/dociler-core/tests/remote.rs), inline Rust unit tests, [PTY harness](scripts/test_tui_pty.py), [CI workflow](.github/workflows/ci.yml) |
| Architecture and rationale | [architecture](docs/architecture.md), [decision records](docs/decisions.md), [release gates](docs/testing-and-release.md) |

## Verification history and resumption

The following is **historical evidence**, not tests rerun for this handoff.
The 2026-09-24 checkpoint records 108 passing regular tests in debug and release
(one additional real-model test ignored by default), plus strict Clippy, fmt,
Rust documentation, Markdown checks, and diff checks.

The opt-in pinned SmolLM2-135M fixture passed real runtime load/generation,
reporting 177,229,824 bytes of server-only peak RSS. This is neither Lite/Pro
qualification nor full-session memory usage. CLI SIGINT was exercised during
runtime preflight; an attempted model-load signal smoke raced a missing-model
failure and is **not proof of cancellation during actual model loading**.
Full process-group limits, real Lite/Pro loads, 8K/16K contexts, native non-Linux
checks, and document quality/citation gates remain unverified.

Start with read-only checks, then use the commands in
[development](docs/development.md) and [testing](docs/testing-and-release.md):

```sh
git status --short --branch
git log -5 --oneline
rustc --version
cargo --version
python3 scripts/check_docs.py
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
git diff --check
```

Rust is pinned to 1.85.1. Earlier sessions needed temporary toolchain/Cargo
caches, which disappeared between turns; inspect the environment instead of
assuming `/tmp` artifacts survive. Use `--offline` only when dependencies are
cached. Loopback integration tests need socket access. Reproduce the optional
real-GGUF test only using the documented opt-in procedure, outside Git; do not
download multi-GB models merely to run the ordinary suite. Native credential
services may be unavailable in headless environments; never add a plaintext
fallback to bypass this boundary.

Recent implementation commits, newest first:

- `9e0384b` — probe signal cancellation and diagnostic RSS reporting.
- `6d367be` — opt-in real-GGUF validation of the pinned runtime.
- `d083439` — diagnostic model-load and generation probe.
- `f38599e` — explicit-consent runtime probe and validation checks.

## Starter message for the next agent

> Read AGENTS.md, README.md, IMPLEMENTATION_PLAN.md, CHECKPOINT.md, and HANDOFF.md,
> then the M4 runtime/security/testing documents. Inspect Git status and preserve
> existing work. Resume the checkpoint's next bounded M4 task; do not reset the
> repository, enable unqualified local chat, or claim v1 is finished. Add an
> in-progress checkpoint entry before substantial edits, verify changes, and
> append a completion/handoff entry with exact passed/failed/not-run results.
> Keep private data and downloaded assets out of Git. Explain any proposed
> reprioritization before changing the approved delivery order.
