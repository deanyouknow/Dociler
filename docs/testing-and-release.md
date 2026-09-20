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
writes. These tests do not qualify a model, accelerator, runtime artifact, or
constrained-memory host.
