# Architecture

## Overview

Dociler is a native Rust application with a controlled local-runtime sidecar.
The public `dociler` executable owns configuration, document access, retrieval,
prompt policy, terminal UI, and API behavior. `llama-server` is an internal
inference implementation detail and is never exposed directly.

```text
User / external harness
          |
          v
+--------------------------- Dociler ----------------------------+
| TUI / CLI       Authenticated API gateway                      |
|       \              /                                        |
|        Session and request orchestrator                        |
|        |         |           |             |                   |
|  Permissions  Skills     Retrieval      Backend adapter         |
|        |         |           |          /              \       |
| Workspace -> extraction -> canonical AST   local         remote |
|              worker          + chunks    llama.cpp    OpenAI API|
+----------------------------------------------------------------+
```

## Component boundaries

### CLI and TUI

Owns onboarding, commands, input editing, Markdown rendering, status displays,
progress, confirmations, cancellation, and the transcript held in memory. It
calls application services and contains no parser or provider-specific logic.

### Session orchestrator

Coordinates a user turn: resolve explicit file references, discover/retrieve
workspace context, choose the long-document strategy, assemble the prompt,
stream inference, validate citations, and update in-memory history. It applies
the same behavior to local, remote, and API-originated requests.

### Workspace and permission service

Canonicalizes the starting directory, discovers eligible files, enforces
ignore/hidden/symlink rules, and mediates every read or write. Write grants are
stored per canonical workspace; each write remains confirmation-gated.

### Extraction worker

Runs out of process with no network access and bounded time/memory/output. It
uses Extractous through a Dociler-owned interface and returns a versioned
canonical document structure. Relaunching the `dociler` executable with a hidden
worker argument is preferred over shipping a second public executable.

### Document and retrieval service

Normalizes extractor output, assigns stable source anchors, performs token-aware
chunking, and builds an in-memory BM25 index. It supports full small-document
context, targeted retrieval, and exhaustive map/reduce operations. It does not
persist extracted text or indexes.

### Prompt and skills service

Embeds versioned built-in Markdown skills at build time. It selects only the
modules required for the current task, inserts trusted system policy before
client instructions, marks documents as untrusted data, budgets tokens, and
creates source identifiers that can be post-validated.

### Backend adapters

Expose a shared streaming-chat interface:

- Local adapter manages a pinned `llama-server` child process, health checks,
  one active alias, cancellation, and graceful shutdown.
- Remote adapter normalizes OpenAI-compatible endpoints, authentication,
  timeouts, streaming, model discovery, and provider errors.

### API gateway

Uses the same orchestrator rather than proxying blindly. It handles bearer
authentication, size/concurrency limits, alias enforcement, skill injection,
document uploads, OpenAI-compatible response translation, and server lifecycle.

### Model/runtime manager

Detects hardware, evaluates memory admission, resolves a pinned asset manifest,
downloads with resume, verifies checksums, installs atomically into the user
cache, and selects a validated backend. It never writes model assets into the
workspace.

## Local inference lifecycle

```text
Select local profile
      |
      v
Check RAM/disk/backend --> insufficient --> explain + offer remote/other tier
      |
      v
Verify/download pinned runtime and GGUF
      |
      v
Spawn llama-server on random 127.0.0.1 port with internal key
      |
      v
Poll health --> register only dociler-lite or dociler-pro
      |
      v
Chat/API requests flow through Dociler gateway
      |
      v
Unload on switch/exit unless authenticated background serving is retained
```

Only one model and one inference slot are active. The child binds to loopback,
uses a generated internal API key, and receives an application alias. Dociler
captures and redacts diagnostics before showing them to general users.

## Document question flow

```text
Prompt + optional @files
      |
      v
Resolve allowed documents -> isolated extraction -> canonical AST
      |
      v
Small input? -- yes --> direct bounded context
      |
      no
      v
BM25 retrieval or exhaustive map stage -> source-labelled chunks
      |
      v
Built-in skill + history budget + chunks + user request
      |
      v
Local/remote inference stream -> citation validation -> rendered answer
```

For summaries or comparisons that require full coverage, the orchestrator maps
over all relevant chunks and reduces structured intermediate summaries. For
ordinary questions it retrieves only the most relevant chunks within budget.

## Process and storage model

- Config: OS-standard user configuration directory; no secrets.
- Secrets: OS credential/keychain service.
- Models and runtimes: OS-standard user cache/data directory.
- Chats, prompts, extracted content, chunks, and indexes: process memory only.
- Exports: a path explicitly chosen and approved by the user.
- Background API state: user-level service registration and non-secret metadata;
  bearer token remains in the credential store.

The M2 foundation implements these boundaries in `dociler-core` with separate
`config`, `paths`, `workspace`, `session`, and `credentials` modules. Configuration
initialization/loading is implemented; grant updates and native credential
adapters are not. The session container has no network, parser, or serialization
dependency in its API. See [Configuration](configuration.md) for the implemented
schema and limits. This foundation is not yet the complete request orchestrator
or document filesystem sandbox.

M3 adds the first remote adapter and native credential adapter. A saved profile
contains no key; it derives an opaque credential identifier used with the OS
store. Each remote client validates the URL, resolves and pins DNS addresses,
constructs a proxy-free/no-redirect Reqwest client, checks model discovery plus
a minimal generation, and parses bounded SSE into the memory-only session.
See [Remote providers](remote-providers.md). The executable now has a thin
Ratatui/Crossterm terminal adapter whose in-memory state owns the transcript,
composer, selected profile, and bounded session. Remote generation runs on one
background thread per turn and sends bounded fragments back over a channel, so
terminal drawing/input are not blocked by ordinary streaming. A
provider-independent controller owns cancellation and session commit. The
remote adapter uses a worker-local current-thread async runtime; cancellation
drops active model-listing, verification-generation, or chat transports even if
the peer has stopped sending. Blocking OS DNS and credential calls are not yet
interruptible. Provider policy, profile installation, and HTTP parsing remain
in `dociler-core`. The same profile service owns non-mutating connection checks,
verified endpoint/model editing, confirmed removal, and verified credential
rotation. Long-running mutations reload settings after verification, merge
unrelated profile changes, and compare the target before save. Authenticated
cross-origin editing requires explicit destination consent before native-key
retrieval. Because filesystem configuration and native credential stores have
no shared transaction, removal/key clearing commits the non-secret state first
and reports orphan-cleanup warnings; adding a key rolls it back if the required
config marker cannot be published.

The terminal refreshes profiles before profile commands and prompt dispatch.
Unrelated changes preserve context; an active-profile change resets the bounded
session and blocks the first pending prompt so routing must be confirmed by
resubmission. Per-session connection state is visible as unchecked, checking,
ready, or attention, with `/connect check NAME` and `/connect edit NAME` recovery
actions. Local onboarding, skills, documents, and local orchestration are still
future layers.

M4 begins with a separate read-only `hardware` service in `dociler-core`. It
collects memory, CPU, instruction, and asset-filesystem facts without process
enumeration or persistence, applies lower Linux cgroup limits when present, and
maps one immutable snapshot to explicit Lite/Pro preflight states. CLI rendering
is kept outside the service so future onboarding and API consumers share the
same admission decisions. Passing means only that a later packaged-runtime probe
may proceed.

The next M4 layer adds an immutable `assets` service. It maps each public model
alias and supported OS/architecture to a commit/tag-pinned source, exact byte
size, SHA-256, and versioned OS-data path. Metadata inspection and full hashing
share this service across the CLI and future onboarding. Cache walking rejects
symlinks and non-regular components and never creates a missing directory. The
currently opened file is hashed in bounded chunks, but future runtime execution
must reopen/revalidate the installed object immediately before use to close the
remaining inspection-to-execution race.

The `downloads` service adds the first M4 mutation path behind explicit caller
consent. It creates only versioned OS-data directories, takes a per-asset OS
file lock, hashes a restrictive managed partial, and resumes with a validated
HTTP range from an immutable manifest URL. It rejects unsafe origin families,
redirect downgrades, inconsistent lengths/ranges, excess bytes, and checksum
mismatches. A verified partial is published with a same-directory no-clobber
hard link, so a concurrent or pre-existing final is never replaced. Partial
bytes remain after interruption for a later resume. The CLI currently invokes
this for the platform runtime archive and chosen model.

The `runtime_install` service reopens and rehashes the pinned archive before
private extraction. It accepts only the manifest-selected tar.gz/ZIP layout,
enforces path, entry-count, individual-file, and total-expanded-size bounds,
rejects duplicates, hard links, device/FIFO entries, encryption, traversal, and
portable Windows-special paths. Official Unix bundles contain relative library
symlinks; Dociler verifies their targets remain within the extracted inventory
and materializes them as ordinary private files, leaving no links in the
installed tree. It records each file's path, size, and SHA-256 in a bounded
inventory, verifies the complete staging tree, then publishes a versioned
runtime directory without replacing an existing invalid install. Inspection can
perform metadata-only or full per-file verification. No code is launched during
installation. The new `runtime_probe` service rehashes that inventory before
each explicit probe, generates an ephemeral internal key, starts only the
inventoried executable with an empty model cache/directory on a random
loopback port, and validates pinned version, health, key enforcement, and
no-model router state. Its child guard always attempts kill/reap on exit, while
the normal path confirms shutdown. Raw output is bounded in memory and not
surfaced by the CLI. This is a lifecycle smoke check only: accelerator
discovery/selection, real model load/generation, and local chat remain pending.

## Failure containment

- Parser crash/timeout affects only the worker and produces a per-file error.
- Local runtime failure leaves the TUI alive with restart/switch diagnostics.
- A malformed remote stream terminates only that turn and keeps unsent history.
- An invalid citation is removed or marked unverified; it is never silently
  presented as a valid source.
- Partial downloads and exports are written to managed temporary paths and
  published only after verification/success. Asset publication is explicitly
  no-clobber; resumable asset partials survive interruption.
