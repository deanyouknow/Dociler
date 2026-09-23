# Security and Privacy

## Default posture

Dociler is local-first, read-only, ephemeral, and network-silent after required
assets are cached. Privacy claims must be enforced by architecture and tests,
not only displayed in the UI.

No telemetry, crash upload, analytics, prompt logging, or remote update payload
may contain workspace paths, hardware identifiers, document metadata, prompts,
answers, or file contents.

The M4 hardware inventory is local, read-only, and ephemeral. It reads only
memory/CPU facts and filesystem capacity for the future OS-managed asset path;
it does not enumerate processes, inspect workspace files, create directories,
persist a device fingerprint, or send inventory over the network. Future update
or asset requests must not attach this inventory.

## Data classification and retention

| Data | Default location | Retention |
| --- | --- | --- |
| Document originals | User workspace | Never copied by Dociler except bounded temporary upload handling. |
| Extracted text/chunks/index | Process memory | Cleared on session end, `/clear`, unload, or cancellation. |
| Chat and prompt context | Process memory | Cleared on session end unless explicitly exported. |
| Models and runtimes | User cache/data directory | Persist until user removes them. |
| Non-secret preferences | User config directory | Persist until reset/uninstall. |
| API keys and bearer tokens | OS credential store | Persist only as required by the saved profile/service. |
| User exports | Explicit user-selected path | User-controlled. |

Temporary upload and export files use restrictive permissions and are removed
on success, error, cancellation, disconnect, and next-start recovery. Managed
asset `.partial` files also use restrictive permissions, but intentionally
survive transfer errors, cancellation, and process exit so a multi-gigabyte
download can resume; they are removed after successful publication or by an
explicit scoped restart/removal operation.

## Local and remote network rules

- Local mode makes no inference or document-content network request.
- Asset and update requests fetch only signed/pinned release metadata and chosen
  artifacts; do not include workspace information.
- For a remote inference profile, show the normalized destination before any
  document content is sent and obtain consent once per session.
- Consent applies only to that destination/profile and current process. A
  redirect to a different origin requires new consent or is rejected.
- Allow plaintext HTTP only to loopback or validated private-network addresses.
  Require HTTPS for public destinations and verify certificates normally.
- Protect against DNS rebinding and redirect-based transition from a private to
  public endpoint or vice versa.

The first M3 remote client enforces the transport subset before any request: it
requires HTTPS except for all-private/all-loopback DNS results, pins resolved
addresses into a proxy-free client, disables redirects, bounds response parsing,
and emits sanitized error classes. See [Remote providers](remote-providers.md).
Chat and profile verification use cancellable async HTTP on isolated generation
workers; dropping a stalled request is covered by a loopback test. OS DNS and
credential-service calls remain blocking platform boundaries. Document content
cannot enter this path yet. Cross-platform DNS/TLS/keychain
runtime validation and redirect/rebinding adversarial coverage remain release
gates rather than implied guarantees.

Profile lifecycle operations never place a secret in settings. Key changes are
verified against the saved endpoint/model before native-store or configuration
mutation. Adding the first key writes the native secret and then publishes the
credential marker, with best-effort rollback if publication fails. Clearing a
key publishes keyless configuration before deleting the old secret. Profile
removal likewise publishes removal before native-key cleanup. Cleanup failures
are reported as possible orphaned credentials rather than restoring a profile
that may no longer be usable. These operations cannot be atomic across an OS
credential manager and the config filesystem; the chosen order keeps saved
profiles usable and avoids silently weakening authentication.

Editing an authenticated profile to a different scheme, host, or effective port
requires explicit destination confirmation. The non-interactive command requires
`--confirm-credential-destination`; the TUI shows the normalized destination and
requires typed `/confirm`. Without confirmation, the core service rejects the
operation before it asks the credential store for the key or opens a network
connection. Model-only and same-origin edits retain the existing policy without
an additional destination prompt.

Profile installation, credential rotation, and endpoint/model editing reload
validated settings after remote verification. Unrelated concurrent profile
changes are merged, while a changed edit/rotation target produces a conflict
instead of being overwritten. The TUI refreshes profiles before commands and
before every prompt. If the active profile changed, it clears model context and
holds the prompt until the user submits again. These checks reduce stale-state
disclosure and overwrite risk, but do not claim a fully serializable transaction
across independent processes and the OS credential manager.

## Filesystem containment

- Canonicalize workspace and candidate paths immediately before access to reduce
  time-of-check/time-of-use gaps.
- Never follow a directory symlink during discovery.
- Permit a selected file symlink only when its resolved target remains within
  the authorized workspace.
- Deny special devices, sockets, named pipes, proc/sys pseudo-files, and network
  filesystem behavior that cannot satisfy regular-file checks.
- Apply read/write checks in the service layer so TUI, non-interactive commands,
  and API routes cannot bypass them.
- The document-upload API is stateless and receives no workspace write tools.

## Document and parser threats

Documents may be malformed, adversarial, compressed bombs, or contain macros,
external links, embedded objects, instructions, and exploit payloads.

- Parse in a short-lived worker with bounded input, expansion, memory, CPU time,
  wall time, output, and metadata.
- Disable worker network access and external resource fetching.
- Ignore macros, active content, scripts, and embedded executables.
- Validate MIME/signature rather than trusting the extension.
- Sanitize worker diagnostics and never deserialize an unbounded result.
- Keep Extractous and its native parser dependencies pinned and dependency-audited.

## Prompt injection and grounding

Treat extracted content as quoted, untrusted evidence. Delimit it separately
from Dociler policy and skills. Documents cannot enable permissions, change
providers, start the LAN server, select hidden files, write files, or redefine
tools.

The answer pipeline only accepts source identifiers supplied by the host in the
current request. Invalid citations are removed or marked unverified. For high
impact numeric/date extraction, require a trace to exact source content and fail
closed when extraction is ambiguous.

These controls reduce but cannot eliminate model prompt injection. Documentation
must not claim a mathematical security boundary inside the LLM.

## Write authorization

- Read-only is the default for every new workspace.
- A write grant is keyed to the canonical workspace identity and explicitly
  enabled through permissions UI.
- Every operation still requires a target-specific preview and confirmation.
- Only MD/TXT may be modified in place; binary source overwrite is hard-denied.
- Write through restrictive temporary files and atomic replacement.
- Keep undo bytes in memory only and warn before ending a session that has
  uncommitted undo history.

## LAN API

- Off by default; bind only after explicit `/turn-on-remote`.
- Use a cryptographically random bearer token stored in the OS credential store.
- Require authentication for all `/v1/*` calls and rate-limit failures.
- Do not provide wildcard CORS, cookies, browser-session auth, or query-string
  tokens.
- Redact absolute paths, internal ports, internal API keys, model filenames,
  prompts, and raw stack traces from responses.
- Enforce bounded concurrency/body size and cancel inference on disconnect.
- Background service registration is per-user, never privileged/system-wide.
- Public-internet exposure, TLS termination, multi-user authorization, tenant
  isolation, and hostile multi-user deployment are outside v1.

## Secrets and configuration

Never store provider keys or bearer tokens in TOML/JSON config, environment
diagnostic output, shell history suggestions, logs, crash reports, or the
repository. Support environment-variable input for automation only when clearly
documented, and never echo the resolved value.

Configuration files use user-only permissions where supported. Unknown or
invalid security settings fail closed. Migrations preserve or strengthen the
prior policy and are covered by tests.

## Supply-chain requirements

- Pin runtime, model, parser, and build dependencies.
- Verify model/runtime size and SHA-256 against a signed release manifest.
- Generate dependency/license inventory and SBOM for releases.
- Build releases from protected tagged commits with provenance.
- Notify about updates but require user confirmation before replacement.
- Never execute an unverified partial download.

The first asset layer embeds `dociler-assets-v1` in the executable and uses only
immutable upstream revisions/tags. Read-only listing checks file metadata;
explicit verification additionally streams exact-length regular files through
SHA-256. Symlinked cache roots/components and non-regular files fail closed.
The download layer requires explicit CLI consent before filesystem/network
mutation, accepts only the built-in HTTPS origin/redirect families, disables
proxies, locks each asset, validates range metadata before appending, caps bytes
at the manifest size, and publishes a verified partial without replacing an
existing final path. Cancellation drops a stalled body request and preserves
the partial. This is not model-load or inference authorization: signed external
update metadata, execution-time revalidation, process isolation, and runtime
probing remain required before local inference can run.

The extraction layer now rehashes the archive immediately before reading it,
uses a private same-filesystem staging directory, and never delegates parsing to
shell archive tools. It rejects unsafe/special entries and decompression limits,
materializes only validated in-root library links as regular files, records
per-file size/SHA-256 inventory, and refuses to replace an invalid installed
tree. Full verification rejects altered, missing, extra, linked, or non-regular
outputs. Extraction alone does not authorize model loading or inference: that
later path must perform final inventory verification, apply process/network
limits, and pass bounded health and generation probes before selection.

The explicit `model runtime-probe --confirm` path now rehashes every inventoried
file immediately before executing only `llama-server`. It launches a model-free
router with empty private model/cache directories, a cryptographically random
key in a 0600 temporary file on Unix, a random `127.0.0.1` port, no Web UI, no
autoload, and no inherited environment. It proves health, authentication,
empty models, and pinned build; raw output is held only in an 8 KiB in-memory
ring. The process is terminated and reaped on success, cancellation, timeout,
or validation failure. The current command is not a model-load or network
sandbox: a compromised verified binary, same-user filesystem race, or local
process able to read same-user private files remains outside its guarantees.
CLI signal cancellation and native non-Linux process behavior still need
validation.

The model-load diagnostic adds explicit consent and repeats full pinned GGUF
hashing, runtime inventory validation, and live hardware preflight before and
after the version check. It refuses experimental Lite and blocked/incomplete
hardware states, uses CPU-only bounded settings, and checks the single-model
alias/path/build before a fixed tiny generation. It holds the answer only in a
bounded in-memory response and does not print or save it. Cancellation and
timeouts stop/reap the child. This is not a hardened process sandbox, a
same-user filesystem-race solution, or permission to route user documents or
chat through the local backend. CLI signal wiring and real-model memory
measurement remain open.

## Threat boundary

v1 protects against accidental remote disclosure, casual LAN access, path
escape, unsafe default writes, malformed document failures, and common prompt
injection paths. It does not protect documents from the logged-in OS user,
malware running with the same privileges, a compromised remote provider, a
compromised model/runtime artifact before verification metadata is trusted, or a
deliberately modified Dociler binary.
