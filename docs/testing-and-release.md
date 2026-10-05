# Testing and Release Gates

## Test layers

### Unit tests

Cover configuration parsing/migration, canonical paths, ignore behavior,
permission lookup, URL normalization and private/public classification, redirect
policy, token budgeting, chunk boundaries, BM25 ranking, prompt ordering, source
ID generation/validation, alias mapping, API error translation, SSE framing,
download resume/hash checks, and atomic write/export planning.

### Golden document fixtures

Maintain small, redistributable fixtures for MD, TXT, PDF, DOCX, DOC, RTF, and
ODT. Each format includes headings, paragraphs, lists, tables, links, Unicode,
Indonesian text, dates, currency, and repeated values. Compare canonical AST and
rendered source anchors to reviewed golden output.

Negative fixtures include image-only PDF, encrypted PDF, malformed/truncated
files, extension/MIME mismatch, decompression bomb, extreme nesting, oversized
metadata, unsupported encoding, embedded macro/object/link, timeout behavior,
and a document containing prompt-injection instructions.

### Integration tests

- TUI/service workflow with a deterministic fake backend.
- Local child startup, health, aliasing, streaming, cancellation, crash recovery,
  model switch, and unload using a tiny test model where appropriate.
- Remote endpoint verification, auth failure, malformed SSE, timeout, redirect,
  disconnect, and per-session document-consent behavior.
- Read-only denial, persistent per-workspace grant, per-write confirmation,
  diff/preview, atomic MD/TXT edit, session undo, and new export.
- API authentication, `/v1/models`, streaming/non-streaming chat, document
  upload cleanup, OpenAPI/router agreement, limits, and error envelopes.

### Platform and installer tests

Run clean installation, upgrade, failed-checksum recovery, uninstall, long paths,
spaces/non-ASCII paths, credential-store access, service start/stop, and first
launch on:

- macOS 13+ arm64 and x86-64;
- Linux glibc x86-64 and arm64;
- Windows 10/11 x86-64.

Release jobs must execute built artifacts on their target OS rather than only
cross-compile them.

## Model-quality corpus

Version a reviewed corpus containing at least:

- exact fact, date, currency, name, and table-cell extraction;
- single-document and multi-document questions;
- contradictions and “not present” answers;
- chronological and thematic summaries;
- cross-document comparison;
- long-document full-coverage map/reduce tasks;
- English and Indonesian prompts/documents;
- prompt injection inside document content;
- citation attribution and deliberately confusable repeated facts.

Run with fixed runtime/model hashes, context/output limits, sampling settings,
skill version, and multiple seeds where sampling remains enabled. Store scores,
hardware, wall time, tokens/second, peak process-group RSS, and failures.

## Stable model gates

| Gate | Lite | Pro |
| --- | ---: | ---: |
| Factual QA accuracy | >= 85% | >= 92% |
| Valid citation rate | >= 95% | >= 97% |
| Invented numeric/date values in deterministic extraction | 0 | 0 |
| Active context in memory run | 8,192 | 16,384 |
| Maximum process-group peak RSS | 5.5 GiB | 11.5 GiB |
| Constrained host RAM | 8 GB | 16 GB |

A memory run includes TUI, parser/index activity, long-document map/reduce, and
sustained generation—not merely model load. It must finish without OOM, runtime
crash, or sustained swap thrashing.

Compare Qwen3.5 primaries with the fixed Qwen3 fallbacks using identical tests.
Use the fallback only when the primary fails and the fallback passes every gate.
If neither passes, do not publish that stable tier.

## Security and privacy tests

- Assert zero network calls during a cached Local session.
- Inspect logs/errors for secrets, prompts, absolute paths, document text, and
  internal runtime details.
- Attempt symlink, traversal, TOCTOU, special-file, hidden/vendor, and API-based
  workspace bypasses.
- Verify a document cannot grant writes, switch backends, or start serving.
- Verify remote consent is destination-bound, per-session, and invalidated by an
  origin-changing redirect.
- Fuzz extraction protocol, API request parsing, path handling, Markdown
  rendering, and canonical AST decoding.
- Audit dependencies/licenses and scan release artifacts and repository history
  for secrets.

## Documentation checks

- All relative Markdown links resolve.
- Each document has exactly one level-one title and logical heading order.
- README links every normative supporting document.
- API examples match the generated OpenAPI contract.
- Model filenames, sizes, hashes, aliases, contexts, and gates agree everywhere.
- `CHECKPOINT.md` contains current status, verification, blockers, next task, and
  an append-only activity entry for the release work.

## Release checklist

1. Working tree and checkpoint are consistent.
2. Formatting, linting, unit, fixture, integration, security, and platform tests
   pass at the exact release commit.
3. Both supported model profiles pass or are explicitly withheld.
4. Installers pass on clean target systems.
5. Runtime/model manifests are pinned and signed.
6. Binaries are signed/notarized where applicable.
7. Checksums, SBOM, license/NOTICE inventory, changelog, and build provenance are
   attached to the release.
8. Upgrade and rollback are tested from the previous stable version.
9. Documentation and `CHECKPOINT.md` identify known limitations honestly.
10. The protected release is published only after all advertised-platform gates
    pass; partial platform support is stated explicitly rather than implied.

## Current baseline

M1 introduces a Rust executable and CLI integration tests covering help/version,
non-interactive startup, invalid arguments, diagnostics, and absence of workspace
writes. M2 adds settings defaults/schema/size/privacy checks, atomic no-clobber
initialization (including concurrent attempts), exact workspace-grant lookup,
Unix symlink/permission negatives, session limits/clearing, credential redaction
and fail-closed unavailability, and CLI initialization/recovery checks. Tests use
temporary paths and child-only config overrides; no developer settings are
modified. M3 adds strict URL/plaintext policy tests; mock OpenAI models,
generation, redirect, malformed/incomplete SSE, response-limit, bearer-auth,
Unicode streaming, profile round-trip, and executable add/list/run coverage.
Terminal unit tests exercise deterministic slash-command transitions, confirmed
clearing, profile switching, unavailable-command behavior, bounded input,
Escape cancellation state, and off-screen rendering through Ratatui's test
backend. Non-terminal executable tests assert that startup emits no control
sequences and creates no workspace state. A provider-independent fake backend
tests ordered fragments, successful session commit, and deterministic cancelled
turns. Profile service tests separate configuration from credentials, prove
pre-cancelled install/removal/rotation writes nothing, exercise native-key marker
rotation, and cover orphan cleanup reporting. Terminal tests cover setup
backtracking, masked key rotation, and exact-repeat removal with active-context
reset. Executable coverage confirms unconfirmed removal and missing rotation
input do not mutate configuration. Refresh/edit/check coverage verifies that
unrelated concurrent changes survive long verification, stale target edits are
rejected, authenticated origin changes stop before key access without consent,
and a prompt is not transmitted after the active profile changes on disk. The
remote integration suite verifies that
cancellation drops a peer-stalled HTTP stream before the transport timeout.
`scripts/test_tui_pty.py` additionally drives the real executable on Linux
through onboarding, two contextual turns, stalled-stream cancellation, a window
resize, a saved-profile health check, alternate-screen restoration, and clean
exit against a loopback mock.
Loopback tests bind only an ephemeral
`127.0.0.1` port. The CI definition runs native builds/tests across five OS/architecture
combinations. See `CHECKPOINT.md` for actual local and hosted verification results;
the existence of a workflow is not evidence that it has run. Later product and
release gates above remain outstanding. Live-provider cancellation, blocking
DNS/keychain interruption, and non-Linux target-platform terminal behavior are
not yet covered.

The first M4 tests use injected inventory values to cover below-6-GB rejection,
6–8-GB experimental Lite classification, decimal 8/16 GB class boundaries,
available-memory and disk failures, successful probe eligibility, and
fail-closed missing data. A live smoke test asserts that inventory creates no
target directory and leaves at least one logical CPU available. CLI integration
coverage verifies that `model status` reports both tiers without workspace
writes. The next asset tests verify manifest uniqueness, immutable HTTPS
sources, explicit platform mapping, missing-cache no-write behavior, separated
metadata/full-hash states, exact-size and SHA-256 mismatch rejection, and Unix
symlink rejection. CLI coverage asserts that list/verify remain read-only and
that missing assets produce a failing verification status. Synthetic five-byte
fixtures exercise hashing; no multi-gigabyte model/runtime artifact is fetched.
These tests do not qualify a model, accelerator, extracted runtime, or
constrained-memory host.

The next M4 download tests use only synthetic 11-byte loopback fixtures. They
cover consent-before-mutation, complete publication, exact range resume, ignored
range rejection without append, checksum failure without publication, verified
cache short-circuiting, stalled-body cancellation, restrictive Unix modes, and
CLI refusal before `--confirm`. No real model/runtime artifact is downloaded,
extracted, or executed; multi-gigabyte resume behavior and native non-Linux
filesystem semantics remain release-gate work.

The runtime-install tests generate local tar.gz and ZIP fixtures and cover
consent-before-mutation, safe-link materialization, hard-link and escaping-link
rejection, required server naming, private executable permissions, exact
inventory verification, modified/extra-file detection, portable traversal and
Windows device-name rejection, and refusal to overwrite an invalid install.
CLI coverage verifies that missing consent creates no state. A smoke check used
the real pinned Linux x86-64 archive in an isolated temporary data directory,
verified its checksum, installed and reverified 60 files/69,015,606 bytes, and
confirmed the tree had no symlinks; no runtime executable was launched. Other
native platforms and hostile decompression stress corpora remain release gates.

The next M4 probe tests cover consent and full-inventory rejection before any
execution, a successful model-free authenticated loopback start/stop, bounded
diagnostic capture under a large output burst, cancellation and timeout with
child reaping, authentication failure, version mismatch, and invalid health.
The exact pinned Linux x86-64 archive also passed an isolated model-free CLI
probe. These tests do not load a GGUF, qualify GPU or memory, exercise native
macOS/Windows behavior, or establish local inference quality.

The regular M4 model-probe tests use a tiny synthetic protocol fixture and
mock `llama-server`. They cover consent and missing/invalid
model rejection before execution, blocked/experimental/incomplete hardware
states, a post-version revalidation barrier, exact alias/path/build/auth
checks, bounded fixed generation, load timeout, stalled-generation
cancellation, and child reaping. The CLI test checks consent and missing-model
refusal without creating asset state.

An opt-in Linux x86-64 integration test uses the actual pinned `b10809`
`llama-server` and a separately downloaded, Apache-2.0
[`SmolLM2-135M-Instruct` Q4_K_M GGUF](https://huggingface.co/bartowski/SmolLM2-135M-Instruct-GGUF/tree/09816acd5d99df7be770d85ea30822623dab342c).
The file is 105,454,432 bytes with SHA-256
`2e8040ceae7815abe0dcb3540b9995eaa1fa0d2ca9e797d0a635ae4433c68c2d`.
Download it into an isolated `DOCILER_REAL_GGUF_TEST_ROOT` at
`data/models/manifest-v1/real-smollm2/SmolLM2-135M-Instruct-Q4_K_M.gguf`;
place and install the pinned runtime archive under that same root using
`DOCILER_DATA_DIR=<root>/data dociler model runtime-install --confirm`.
Run `DOCILER_REAL_GGUF_TEST_ROOT=<root> cargo test -p dociler-core
real_smollm2_fixture_loads_and_generates --locked -- --ignored` on a host
that permits private loopback binding. The test verifies fixture SHA-256 and
full runtime inventory before execution, repeats both before model spawn,
checks the exact alias/path/build/auth contract, makes one bounded generation
request, and reaps the child. It is a protocol fixture only, never a Dociler
Lite/Pro profile or quality substitute. Do not commit the GGUF or runtime.

The pinned Linux x86-64 runtime passed these opt-in tests (load/generation,
memory ceiling breach abort, and loading cancellation) on 2026-10-05. Real
pinned Lite/Pro loads, process-group peak RSS, 8K/16K contexts, CPU/GPU target
matrix, signal cancellation, model quality, and local chat remain unverified
release gates.

The diagnostic safety unit adds a scoped CLI signal listener and Linux
child-only high-water RSS reporting. Its automated Linux tests send SIGINT and
SIGTERM to a separate process (never the test runner) and check token
cancellation; memory parser/current-process tests reject malformed units and
require a nonzero kernel reading. A pinned real 135M GGUF probe produced a
177,229,824-byte server-only high-water RSS on one Linux x86-64 host; that
figure is not a supported-tier budget. A scriptable model-free CLI probe
also returned cancellation after SIGINT during preflight; no server was left
running. Deterministic unit tests verify that asset verification and installed
runtime inventory hashing stop immediately when the cancellation token is
tripped, returning cancelled status without publishing or caching unverified files.
The process-group peak memory accounting unit adds concurrent process-group
RSS sampling (Dociler host process, sidecar child process, and any sidecar child
descendants via `/proc/<pid>/task/<pid>/children`) and an enforceable memory
ceiling guard. Deterministic Linux tests verify that non-labeled memory units are
rejected (`parses_only_labeled_memory_units`), combined current process RSS is
observed (`observes_current_process_and_combined_memory`), ceiling breaches
trip cancellation and report structured abort details
(`memory_ceiling_breach_triggers_cancellation_and_abort`), the model probe
aborts cleanly and reaps the child process upon ceiling breach
(`memory_ceiling_breach_aborts_probe_and_reaps_child`), and experimental or
incomplete hardware preflight states fail closed without explicit opt-in
(`hardware_gate_rejects_experimental_and_incomplete_states`). The opt-in real
SmolLM2 135M fixture test additionally verifies that process-group peak RSS is
reported and strictly greater than zero alongside server-only peak RSS. Additional
opt-in real GGUF tests verify that setting an unmeetable memory ceiling aborts cleanly
and reports `MemoryLimitExceeded` (`real_smollm2_fixture_aborts_on_memory_ceiling_breach`),
and that pre-cancellation safely halts probe execution without orphaned server
sidecars (`real_smollm2_fixture_loading_cancellation_stops_server`). These
checks establish measurable memory telemetry and active ceiling enforcement
during diagnostic probing; full-context 8K/16K extended workloads, multi-GB
pinned Lite/Pro qualification, and native non-Linux memory telemetry remain
outstanding release gates.

The cache lifecycle and cancellation safety unit adds model removal, model
repair, download signal cancellation, and loading-phase probe cancellation.
Deterministic tests verify that model load cancellation during server startup
aborts cleanly and reaps the sidecar (`loading_cancellation_reaps_the_child`).
Core asset removal tests verify that cache removal requires explicit consent
(`remove_cached_model_requires_consent`), deletes target final, partial, and
lock files while preserving unrelated profiles
(`remove_cached_model_cleans_files_and_preserves_unrelated`), removes runtime
archives and installed directory trees
(`remove_cached_runtime_removes_archive_and_installed_tree`), and rejects
symlinks fail-closed (`remove_cached_assets_rejects_symlinks`). CLI integration
tests verify that `model remove` and `model repair` dry-run safely without writes
when unconfirmed and execute correctly when confirmed. Furthermore, CLI integration
tests verify that `model load-probe` recognizes `--experimental` and `--ceiling-bytes <BYTES>`
(and inline `--ceiling-bytes=BYTES`), requires `--confirm` consent, rejects invalid
or zero values with usage exit code 2, and preserves arbitrary flag ordering
(`model_load_probe_supports_experimental_and_ceiling_bytes_flags`).
