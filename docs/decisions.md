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

## ADR-014: Native event-loop TUI with isolated generation

**Decision:** Implement the first terminal surface with pinned Ratatui 0.29 and
Crossterm 0.28, preserving separate non-interactive commands. Keep terminal
events/rendering on the main thread and move each remote generation to a worker
thread connected by an in-process channel. Hold transcript, prompt,
partial output, and model session only in memory, with best-effort zeroization.

**Why:** These versions retain the Rust 1.85 baseline, support deterministic
off-screen rendering tests, and restore terminal state through a small native
adapter. A background worker keeps streamed text and cancellation input
responsive. The provider-independent controller owns session commit, and a
current-thread Tokio runtime confined to each remote worker makes model
verification and generation transports abortable without a process-global
runtime. Refusing the TUI when stdin/stdout are not terminals keeps automation
output stable and free of control sequences.

**Rejected:** Replacing scriptable commands with a terminal-only UI, persisting
transcripts, silently falling back from failed TUI setup, and introducing a
browser/Electron frontend. Blocking DNS and OS credential calls remain explicit
platform boundaries rather than reasons to weaken endpoint or secret policy.

## ADR-015: Verify-first profile changes with explicit cross-store cleanup

**Decision:** Re-load validated settings and verify a candidate authentication
policy before changing it. Profile removal and key clearing publish the
non-secret configuration first, then delete the native credential. If cleanup
fails, keep the safe configuration result and report a possible orphaned secret.
When adding a key to a formerly keyless profile, write it to the native store,
publish the required marker, and best-effort roll the key back if publication
fails. Require explicit confirmation for destructive non-interactive commands
and exact command repetition in the TUI.

**Why:** Configuration files and platform credential managers do not share an
atomic transaction. This ordering avoids leaving a saved profile that demands a
missing key, never silently falls back to unauthenticated access, and makes the
remaining cleanup risk visible. Verification prevents committing a replacement
that cannot list and minimally invoke the configured model.

**Rejected:** Deleting the key before its profile/marker, silently ignoring
cleanup failures, storing recovery copies in configuration, replacing profiles
without verification, and one-keystroke destructive removal.

## ADR-016: Refresh before disclosure and compare verified profile mutations

**Decision:** Reload saved profiles before terminal commands and every prompt.
Preserve the conversation when only unrelated profiles changed; reset context
and require prompt resubmission when the active destination, model, or credential
policy changed. After a long remote verification, reload settings again, merge
unrelated changes, and reject an edit or credential rotation if its target no
longer matches the verified starting state. Require explicit confirmation before
retrieving or sending an authenticated profile's existing key to a different
scheme, host, or effective port.

**Why:** A terminal session can outlive changes made by another Dociler process.
Routing a queued prompt with stale assumptions risks disclosure, while blindly
saving verified stale state risks overwriting another user's update. Explicit
cross-origin key consent makes the credential destination visible at the point
it changes. Individual config publication remains atomic, but this policy does
not claim full multi-process serialization across network checks, files, and OS
credential services.

**Rejected:** Trusting the startup snapshot indefinitely, automatically sending
the first prompt after an active-profile refresh, last-writer-wins target edits,
and silently reusing a stored bearer key at a different origin.

## ADR-017: Fail-closed hardware preflight before runtime admission

**Decision:** Collect a local, ephemeral RAM/CPU/disk snapshot through pinned
`sysinfo` 0.33.1 system/disk features, without enumerating processes. Apply a
lower Linux cgroup memory limit when present. Classify Lite and Pro as blocked,
experimental, inventory-incomplete, or ready for a required runtime probe; never
call a static preflight final admission. Use decimal 6/8/16 GB hardware classes,
the 5.5/11.5 GiB qualification working-set floors, and provisional model-size
plus 2 GiB disk floors. Keep accelerator candidates unselected until a packaged
runtime load/generation probe succeeds.

**Why:** Hardware marketing classes and binary memory budgets use different
units, and container limits can be lower than host RAM. Explicit states avoid
offering Pro merely because its file fits or promising acceleration from a
driver/library hint. A small shared service gives doctor, future onboarding, and
the API one policy without persisting a hardware fingerprint.

**Rejected:** Comparing 8/16 GB classes as 8/16 GiB, treating swap as available
admission memory, enumerating processes, shelling out to platform utilities,
selecting CUDA/Metal/Vulkan without a runtime probe, and downloading assets as a
side effect of diagnostics.

## ADR-018: Built-in immutable asset manifest before download support

**Decision:** Compile `dociler-assets-v1` into the executable with exact model
repository revisions and the exact llama.cpp semantic release, binary build,
commit, per-target archive URL, byte count, and SHA-256. Keep metadata listing
separate from explicit full-file verification. Store each artifact beneath a
versioned model/runtime namespace, reject symlink/non-regular cache components,
and make both operations read-only. Support only the five initial CPU/Metal
release targets until other packages pass platform qualification.

**Why:** Mutable branches, `latest`, rounded sizes, and filename-only checks are
not adequate trust inputs for executable/model artifacts. A built-in manifest
has the same authenticated distribution boundary as the Dociler binary and can
be tested before any network mutation exists. Separate listing avoids hashing
multi-gigabyte files during routine status checks, while explicit verification
gives automation a nonzero result for missing or invalid cache state.

**Rejected:** Fetching metadata from mutable upstream heads at runtime, treating
presence/size as verified, following cache symlinks, silently repairing invalid
files, enabling unqualified GPU bundles, or claiming the archive is executable
before safe extraction and a load/generation probe are implemented.

## ADR-019: Resumable, locked, no-clobber asset publication

**Decision:** Require explicit consent before creating cache state or connecting.
Download only a built-in immutable asset URL through allowlisted HTTPS redirect
families with proxy use disabled. Keep bytes in a restrictive, versioned
`.partial` sibling; hash existing bytes before requesting an exact range; reject
inconsistent status/range/length or excess bytes; and retain failures for
resume. Serialize writers with an OS file lock. After exact size and SHA-256
verification plus file sync, publish with a same-directory hard link that fails
if the final path exists, then remove the partial. `--restart` may delete only
the identified managed partial. Downloading never extracts or executes an
artifact.

**Why:** Multi-gigabyte artifacts need reliable resume and cancellation, while
runtime/model supply-chain data must fail closed. Rename-over-existing semantics
differ by platform and could replace a valid or user-owned final; no-clobber
link publication makes that race explicit. Persistent lock files are harmless,
while kernel-held locks release after crashes and avoid stale-lock deletion
races.

**Rejected:** Implicit first-run downloads, arbitrary user-supplied asset URLs,
ambient HTTP proxies, appending after a server ignores `Range`, deleting partials
on ordinary failure/cancellation, filename/size-only trust, overwriting invalid
finals, extracting archives in the downloader, and executing immediately after
download.

## ADR-020: Bounded extraction with a link-free inventoried runtime tree

**Decision:** Reopen and rehash the immutable archive before extraction. Parse
tar.gz and ZIP in-process with strict entry/path/expanded-size limits and private
same-filesystem staging. Reject duplicate, absolute, traversal, hard-link,
encrypted, device, FIFO, sparse, and otherwise unsupported entries. Because the
official macOS/Linux bundles contain relative shared-library symlinks, accept
only links whose portable relative targets resolve to regular files within the
same extracted inventory, then materialize their bytes into regular files.
Record every installed file's path, size, and SHA-256 in a bounded private
inventory; require the expected server file; verify staging before publication;
and never overwrite an existing invalid runtime directory. Installation does
not execute or probe the runtime.

**Why:** Blind library extraction APIs can permit traversal, link escapes,
special files, decompression bombs, or silent duplicates. Rejecting every
symlink would also reject all four pinned official Unix bundles. Safe
materialization preserves the loader-visible filenames without retaining link
semantics, while the inventory gives later inspection and launch code a closed,
tamper-evident set to revalidate.

**Rejected:** Shelling out to `tar`/`unzip`, using archive-library convenience
unpack methods, preserving symlinks, accepting hard links or special files,
trusting the archive checksum without inventorying output, overwriting damaged
installs, broad recursive cleanup of unknown paths, and launching immediately
after extraction.

## ADR-021: Ephemeral authenticated no-model runtime probe

**Decision:** Require explicit execution consent and a fresh full installed-file
inventory check before spawning the pinned `llama-server`. Probe its exact build
first, then run it only as a no-model router on a random loopback port with a
cryptographically random key file, empty private model/cache directories,
cleared ambient environment, one future inference slot, conservative threads,
disabled UI/autoload, and bounded in-memory diagnostics. Require public health,
unauthenticated rejection, an authenticated empty model list, and pinned
router build/state. Kill and reap the child on every outcome. Do not persist a
backend choice or expose the child port through Dociler's public contract.

**Why:** Safe extraction is not evidence that a platform binary runs or that
its internal HTTP surface enforces authentication. A model-free probe isolates
the lifecycle/transport boundary from multi-gigabyte model load, while making
the later load/generation admission gate explicit. The OS-selected port has a
small reservation-to-bind race; a private key challenge and exact response
checks mitigate, but do not erase, same-user attack assumptions.

**Rejected:** Launching as an implicit side effect of install/list/doctor,
using a known port, inheriting model/provider/proxy environment, putting the key
in command-line arguments, treating `/health` alone as sufficient, retaining a
background process after a diagnostic, or declaring local inference ready from
this probe.

## ADR-022: Diagnostic-only pinned model load before local chat

**Decision:** Permit an explicitly confirmed, short-lived CPU-only model-load
probe only after exact pinned GGUF hashing, full installed-runtime inventory
validation, and fresh supported-tier hardware preflight. Repeat these checks
after the server version check and before listening spawn. Use one model/slot,
a 1,024-token context, small fixed batches, conservative threads, an internal
key on random loopback, exact alias/path/build checks, and one bounded
non-thinking generation. Attempt stop/reap on every result and report any
unconfirmed shutdown; do not retain a backend selection or enable local chat
from this diagnostic.

**Why:** A model-free router probe does not test whether selected weights load
or whether the pinned server can generate with them. A small diagnostic
separates that protocol/lifecycle step from expensive full-context memory and
quality qualification. Experimental 6–8 GB Lite is refused until it has a
measured admission policy. Regular tests use a synthetic mock; a separate
opt-in tiny real-GGUF test verifies only the basic pinned server contract.

**Rejected:** Implicit model load during `doctor`/`model status`, trusting a
prior cache hash or hardware snapshot, automatic GPU offload, exposing the raw
server, persisting the probe process, printing test output, or treating a tiny
generation as proof that 8K/16K contexts and RSS/quality gates pass.

## ADR-023: Scoped probe signals and honest child-only memory telemetry

**Decision:** For explicitly confirmed scriptable runtime/model probes, register
a short-lived signal listener before presenting the probe-start message. Map
Unix SIGINT/SIGTERM and Windows Ctrl+C/Ctrl+Break to the existing cooperative
cancellation token, and let the core controller stop/reap the sidecar. On Unix,
place the sidecar in its own process group so terminal Ctrl+C reaches Dociler
without racing a direct sidecar signal exit. On Linux, observe the live server's
kernel `VmHWM` high-water RSS and report it as a server-only diagnostic or
“unavailable”; do not use it as a release admission result or memory cap.

**Why:** A signal must not bypass cleanup or turn into a successful probe.
Partial RSS from one short-lived process is useful for validating the observer
but excludes Dociler, descendants, document work, GPU memory, full context, and
sustained generation. Labeling it as process-group peak would overstate the
evidence for the 8/16 GB product targets.

**Rejected:** Exiting directly from a signal callback, interpreting missing
`/proc` metrics as zero, imposing an unqualified virtual-memory limit on a
memory-mapped GGUF, or declaring Lite/Pro qualified from a tiny-model RSS.

## ADR-024: Cooperatively cancellable multi-gigabyte asset and inventory hashing

**Decision:** Wire the cooperative `CancellationToken` into all model artifact and
installed runtime verification loops. In `inspect_cached_asset_cancellable` and
`inspect_installed_runtime_cancellable`, poll the cancellation token on every read
buffer chunk (64 KiB) during SHA-256 computation and between file inventory entries.
When cancelled, return explicit `CacheState::Cancelled` and
`RuntimeInstallState::Cancelled` states rather than treating interruption as
missing, corrupt, or verified. Map cancellation in CLI `model verify` and probe
workflows to clean exit codes without spawning processes or publishing unverified
files.

**Why:** Hashing 2.5 GB to 10 GB model files takes significant time on CPU/storage.
Without chunk-level cancellation checks, terminal signals (SIGINT/Ctrl+C) or probe
cancellations are delayed until the entire multi-gigabyte hash completes. A
cooperative check within the buffer loop aborts immediately, releases file
descriptors and locks, avoids spinning CPU, and guarantees that cancelled
inspections are never mistakenly published or cached as valid.

**Rejected:** Long-lived unskippable hash loops, spawning threads that are abandoned
or detached upon signal, treating cancelled verification as asset corruption (which
could prompt unwanted re-downloads or repairs), or bypassing SHA-256 integrity checks
when uncancelled.

## ADR-025: Process-group peak RSS accounting and enforceable constrained-host memory ceiling guard

**Decision:** Sample concurrent resident set size (RSS) across the entire process
group—including the Dociler host process (`parent_pid`), the sidecar server
(`child_pid`), and any descendant tasks spawned by the sidecar (discovered via
`/proc/<pid>/task/<pid>/children` on Linux)—rather than reporting only the sidecar's
kernel `VmHWM`. Provide an enforceable memory ceiling guard (`memory_ceiling_bytes`)
during model probing that trips cooperative cancellation and deterministically stops
and reaps child processes immediately upon breach. Enforce strict fail-closed
hardware admission: experimental 6–8 GB Lite tiers require explicit opt-in
(`allow_experimental = true`) and are otherwise refused, while insufficient or
incomplete hardware configurations always fail closed. On non-Linux platforms where
procfs process-group traversal is unavailable, report process-group peak RSS as
`None` (“unavailable”) rather than returning zero or misleading partial readings.

**Why:** A single process `VmHWM` metric only measures the sidecar's individual peak
memory, completely blind to Dociler's memory footprint, sidecar worker threads or
child processes, document buffers, and concurrent tasks. On constrained hosts
(e.g., 6–8 GB or tight memory containers), unmonitored model loading risks
triggering an operating system Out-Of-Memory (OOM) kill that terminates processes
abruptly without running cleanup handlers, releasing temporary resources, or
producing actionable diagnostics. Combining concurrent RSS sampling with an active
ceiling guard ensures early detection, clean cooperative cancellation, and
deterministic process reaping before host memory limits are breached.

**Rejected:** Relying on single-process `VmHWM` as the sole qualification metric,
guessing or reporting zero for unavailable metrics on non-Linux platforms,
silently admitting the experimental 6–8 GB Lite tier without explicit opt-in,
allowing lingering background sidecars upon memory limit breaches, or trusting
untested memory limits without active enforcement.

## ADR-026: Explicit model cache removal, repair, and cancellable download lifecycle

**Decision:** Provide explicit scriptable `model remove TARGET --confirm` and
`model repair PROFILE --confirm` commands (where TARGET is `dociler-lite`,
`dociler-pro`, `runtime`, or `all`). Without `--confirm`, both commands operate
strictly in dry-run preview mode, reporting exact candidate paths, file sizes,
and integrity states without performing any filesystem mutation. Model removal
safely purges final GGUF files, active `.partial` streams, and advisory lock
files, pruning empty versioned parent directories while verifying path safety,
refusing symlinks, and never modifying unrelated cached profiles or shared
runtimes. Model repair inspects both the model GGUF and the runtime installation:
if files are corrupted (size/SHA-256 mismatch) or missing, it purges invalid
files and re-downloads or re-installs them through the verified pipeline; if
already verified, it reports healthy without re-downloading. Wire
`CliSignalGuard` into `model download` and `model repair` so terminal interrupts
(SIGINT/SIGTERM/Ctrl+C) cooperatively cancel active HTTP body streams and
preserve `.partial` bytes for subsequent resumption.

**Why:** Download errors, interrupted writes, filesystem bitrot, or accidental
tampering can leave invalid files that permanently block subsequent downloads
under Dociler's strict `ExistingInvalid` no-clobber policy. Without explicit
removal and repair tools, users would be forced to manually locate and delete
files inside internal cache directories, risking deletion of unrelated assets
or breaking directory structures. A preview-first dry-run with explicit
confirmation prevents accidental multi-gigabyte deletions while guaranteeing
safe cleanup and restoration. Wiring signal cancellation into downloads ensures
that user interruptions stop network traffic immediately, release locks, and
keep partial files safely resumable.

**Rejected:** Silently overwriting or auto-repairing corrupt cache files on
standard download; deleting unrelated cached profiles or the shared runtime
during single-model removal; following symlinks during cache pruning; leaving
stale lock files after cancellation; or omitting dry-run previews before
destructive operations.

## ADR-027: Interactive TUI local model profile selection, inspection, and cache lifecycle safety

**Decision:** Enable the interactive `/model` command surface in the terminal user interface (TUI) with subcommands: `/model` (or `/model list`), `/model status`, `/model info [PROFILE]`, `/model use PROFILE`, `/model unload`, `/model remove TARGET`, and `/model repair PROFILE`. Selecting a local profile (`/model use PROFILE`) sets the active target to Local, updates the header and status bar to reflect diagnostic-only status, resets conversation context, and explicitly blocks chat prompting with actionable guidance to CLI diagnostic commands (`dociler model load-probe PROFILE --confirm`) or `/connect NAME`. Switching to a remote profile via `/connect NAME` switches backend back to Remote and resets context. Destructive cache operations in the TUI (`/model remove` and `/model repair`) require exact command repetition following a dry-run preview before executing filesystem mutation, matching the safety model of `/connect remove`.

**Why:** Users in the interactive terminal experience need the ability to inspect local profile status, view hardware admission details, inspect model metadata, safely manage cache space, and switch between remote profiles and local diagnostic profiles directly from the TUI without dropping to the shell. At the same time, local inference/chat must remain strictly disabled until full-context memory, process-group peak RSS, and model-quality qualification gates pass. Providing clear diagnostic feedback and fail-closed prompting prevents premature local chat use while delivering the complete model management experience. Requiring exact command repetition for removal and repair in the TUI protects users against accidental multi-gigabyte cache deletions.

**Rejected:** Silently enabling local chat before release qualification gates pass; omitting exact repetition confirmation for cache deletions in the interactive interface; allowing uncoordinated state switches without resetting in-memory conversation context; hiding hardware admission status from the terminal interface.

## ADR-028: Compiled per-target runtime inventory and CPU-instruction release admission gate

**Decision:** Define compiled, immutable release manifests on `RuntimeAsset` across all five official targets (`macos-aarch64-metal`, `macos-x86_64-metal`, `linux-aarch64-cpu`, `linux-x86_64-cpu`, `windows-x86_64-cpu`) specifying `required_cpu_features`, `recommended_cpu_features`, `expected_server_file`, `expected_shared_libraries`, and `expected_file_count_range`. Hardware preflight (`HardwareInventory::evaluate_cpu_instructions`) evaluates host CPU capabilities against target requirements and reports satisfaction or exact missing instructions. Runtime probe and model probe commands (`dociler runtime probe` and `dociler model load-probe`) execute this instruction compatibility preflight early before expensive multi-gigabyte GGUF hashing or runtime process spawning, returning `UnsupportedCpuInstructions` if required instructions (e.g. AVX/AVX2 on x86_64, NEON on aarch64) are missing. Runtime installation verification (`verify_install_at`) enforces that installed trees contain the exact expected server binary, all required shared libraries (e.g. `libllama.so`, `libggml.so`, `ggml-metal.metal`), and file counts within the target's compiled range.

**Why:** Local model execution and llama.cpp runtime binaries rely on specific vector and SIMD CPU instruction set extensions (such as AVX/AVX2 on x86_64 or NEON/Metal on Apple Silicon). Attempting to execute server binaries or load GGUF weights on CPUs lacking these instructions results in fatal SIGILL (Illegal Instruction) crashes, ungraceful aborts, or wasted computing time spent computing SHA-256 digests over multi-gigabyte files that cannot run. Furthermore, incomplete extractions or missing dynamic libraries produce ungraceful runtime dynamic loader failures. Evaluating compiled CPU instruction manifests early in hardware inventory preflight and enforcing strict shared-library inventories fail closed safely with clear user guidance before any process execution.

**Rejected:** Blindly attempting runtime child execution and catching SIGILL or dynamic loader crashes; running multi-gigabyte model hash verification on CPUs known in advance to lack required instructions; allowing runtime installations with missing required shared libraries to verify; omitting platform-specific CPU instruction requirements from the compiled runtime metadata.

## ADR-029: Configurable memory ceiling and experimental tier flags for diagnostic model load probe

**Decision:** Expose `--experimental` (alias `--allow-experimental`) and `--ceiling-bytes <BYTES>` (alias `--memory-ceiling-bytes <BYTES>`) command-line flags on `dociler model load-probe PROFILE --confirm`. The experimental flag explicitly permits probing 6–8 GB hardware tier profiles (e.g. `dociler-lite` on hosts evaluated as `ExperimentalForRuntimeProbe`) that are otherwise blocked by default preflight admission gates. The `--ceiling-bytes` flag allows operators and automated verification suites to enforce an explicit process-group peak RSS limit (covering the Dociler host process and the spawned llama.cpp sidecar process concurrently) during the 1024-token diagnostic load probe; if process-group RSS breaches this ceiling, the probe immediately issues cooperative cancellation, terminates the child process group cleanly, and reports `MemoryLimitExceeded` with exact limit and observed bytes. Invalid ceiling values (such as zero or non-numeric strings) are rejected at argument parsing time.

**Why:** The Dociler hardware preflight gates experimental 6–8 GB systems to protect constrained hosts from out-of-memory kernel kills. However, developers and users evaluating Lite models on borderline systems need an intentional, opt-in mechanism to test whether their specific configuration can safely load the diagnostic context without crashing the machine. Similarly, while default memory qualification uses pre-configured target limits, CI environments, automated qualification tests, and constrained containers require deterministic limits and early abort capabilities. Exposing these controls on the CLI enables thorough testing and user verification while preserving fail-closed defaults and strict `--confirm` consent requirements.

**Rejected:** Silently allowing experimental hardware to run without explicit `--experimental` flag; omitting process-group memory ceilings from CLI execution; relying on kernel OOM killer instead of graceful process-group monitoring and termination; allowing zero-byte memory ceiling limits; enabling local chat when load probe succeeds with experimental flags.

## ADR-030: Canonical document AST representation, format signature validation, and safe workspace discovery

**Decision:** Establish the foundational document model and safe workspace discovery system in `crates/dociler-core/src/document.rs` and `crates/dociler-core/src/workspace.rs`, exposed via the `dociler files [PATH]` command and `/files` TUI slash command:
1. **DocumentFormat & Capabilities:** Define supported formats (`Markdown`, `PlainText`, `Pdf`, `Docx`, `Doc`, `Rtf`, `Odt`) with capability flags for direct in-place editing (`is_direct_editable`: `.md`, `.txt`), new exports (`supports_export`: `.md`, `.txt`, `.docx`, `.rtf`, `.odt`), and legacy input-only restrictions (`is_legacy_input_only`: `.doc`).
2. **Signature & Encoding Validation:** Perform magic-byte sniffing (`%PDF-`, `{\rtf`, OLE compound headers, ZIP archive containers) and strict byte-level pre-validation before parsing. Reject empty files, non-UTF-8 text, null bytes in text files, and password-protected/encrypted PDF files early.
3. **Canonical AST Model:** Define the immutable Dociler AST (`Document`, `DocumentMetadata`, `DocumentSource`, `Block`, `ListItem`, `InlineRun`, `SourceAnchor`). The AST contains only semantic elements (Headings, Paragraphs, Lists, Tables, Code fences, Page breaks, Unsupported markers) and explicit page/section/line-range source anchors. It excludes layout coordinates, embedded scripts, external references, and macros. Provide safe, native in-process parsers for direct-editable formats (`.md` and `.txt`).
4. **Safe Workspace Document Discovery:** Implement gitignore-aware workspace document discovery starting from the canonical root directory. The traversal automatically excludes hidden files and directories (`.*`), default build/vendor/cache directories (`.git`, `target`, `node_modules`, `dist`, `build`, `.cache`, `vendor`, `.gemini`, `models`), and respects nested `.gitignore` rule hierarchies including negation patterns (`!pattern`).
5. **Path Containment & Symlink Guards:** Enforce strict containment safety: directory symlinks are never followed (`skipped_symlinks_count`); file symlinks are canonicalized and verified to point within the workspace root (`starts_with(&self.root)`), immediately rejecting and skipping escaping symlinks. File size limits (default 50 MiB per file) and aggregate size limits (default 250 MiB) prevent denial-of-service from oversized or malicious inputs.

**Why:** Documents are untrusted inputs that must never be able to execute code, escape workspace sandbox boundaries, or exhaust host memory. Extracting and analyzing documents requires a normalized, memory-efficient representation where semantic structure and source citations can be tracked precisely. Parsing direct-editable formats (`.md`, `.txt`) natively in-process provides maximum speed and safety without external process overhead, while format capability flags prevent destructive writes to binary or legacy documents.

**Rejected:** Allowing directory symlinks to be traversed; following symlinks pointing outside the workspace; accepting claimed file types without signature sniffing and encoding validation; parsing binary office documents in-process without sandboxing; allowing layout coordinates or executable macros into the canonical AST; omitting gitignore and build directory exclusions during workspace scanning.
