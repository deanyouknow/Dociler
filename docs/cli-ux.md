# CLI and Terminal Experience

## UX principles

- Lead with a usable choice, not infrastructure terminology.
- Explain privacy and resource consequences at the moment they matter.
- Make current workspace, backend, model, and permission state continuously
  visible.
- Keep expert controls discoverable without requiring them during onboarding.
- Never leave a long download, parse, or generation looking frozen.

## Launch behavior

`dociler` with an interactive terminal opens the TUI in the current working
directory. `dociler chat [PROFILE]` does the same and may select a saved remote
profile. A non-interactive bare invocation prints command help rather than
emitting terminal control sequences. `dociler run PROFILE` handles scripted
one-shot use by reading one prompt from standard input.

The landing header shows:

- `Dociler` version and update state;
- normalized workspace path;
- number of supported documents found;
- total/available RAM and selected acceleration;
- active or last-used backend and model profile;
- read-only or write-enabled workspace status.

During the M3 transition, the working TUI supplies a scrollable plain-text
transcript, bounded multiline composer, memory-only multi-turn history, visible
workspace/profile/privacy state, streaming output, cooperative cancellation,
and the core `/help`, `/status`, `/connect`, `/clear`, and `/exit` commands.
`/connect` can list, refresh, switch, add, check, edit, remove, and update keys
for profiles. The header shows unchecked, checking, ready, or attention state;
`/status` gives a concrete check/edit recovery command. Enter sends, Alt+Enter
inserts a newline, PageUp/PageDown scroll, and Escape cancels active generation
or clears idle input. Ctrl+C cancels while busy and exits while idle.

The real Linux executable is exercised through a pseudo-terminal against a
loopback mock provider for first-run setup, contextual streaming, cancellation,
resize survival, alternate-screen restoration, and clean exit. This automated
coverage does not replace native macOS/Windows or human usability review.

When no profile exists, the current M3 TUI opens a four-step remote setup for
profile name, endpoint, model ID, and an optional hidden API key. `/connect add`
opens the same flow later. It validates and saves only after model listing and a
minimal generation pass; the key can be persisted only in the OS credential
store. `/back` revisits the previous field after a validation or verification
failure, while `/cancel` or Escape stops setup. Escape requests cancellation
before configuration commit when verification is active.

`/connect refresh` reloads profiles saved by another process. Other profile
commands refresh automatically, as does every prompt before transmission.
Unrelated profile changes preserve context. If the active endpoint, model, or
credential policy changed, Dociler clears model context and refuses that first
prompt so the user can review `/status` and deliberately submit it again.
`/connect check NAME` verifies the saved endpoint, native credential, model
listing, and minimal generation without changing settings. `/connect edit NAME`
prefills endpoint/model fields and verifies the candidate before committing it.
An authenticated edit to a different origin displays the normalized destination
and requires typed `/confirm` before Dociler retrieves or sends the existing key.

It does not yet provide the local/backend choice, hardware summary, Markdown
rendering, command/file completion, document discovery, or context metering.
Planned commands report that they are unavailable rather than silently doing
nothing. Chat and verification HTTP operations use cancellable async transport;
blocking OS DNS resolution and credential-service calls remain bounded only by
their platform behavior. Scriptable profile setup and one-shot chat remain
available through [Remote providers](remote-providers.md).

## First-run wizard

The remote half of this wizard is implemented in M3. M4 now provides read-only
hardware/preflight reporting through `model status`, pinned cache presence
through `model list`, and explicit checksum verification through `model verify
[PROFILE]`. The scriptable `model download PROFILE --confirm [--restart]`
command downloads the current-platform runtime archive followed by the selected
model, resumes a managed partial by default, and publishes only after exact size
and SHA-256 verification. `--restart` explicitly discards only that managed
partial. Repair/removal, runtime probes, progress inside the TUI, and the TUI
local choice remain pending. A profile-free launch therefore still
proceeds directly to remote setup; Escape cancels without saving.

After a runtime archive is cached, `model runtime-install --confirm` performs
bounded private extraction and shows the resulting inventory size and
`llama-server` path. The command never runs that executable. `model list` shows
the installed-tree metadata state, while `model verify` performs full per-file
inventory checks when an installed tree exists.

### Backend choice

Use plain labels:

- **Run locally — private after download**
- **Connect to a model API**

### Local choice

- **Dociler Lite — recommended**: Qwen3.5 4B Q4_K_M, 3.01 GB download,
  8 GB-class target, 8K working context.
- **Dociler Pro — best quality**: Qwen3.5 9B Q4_K_M, 6.17 GB download,
  16 GB-class target, 16K working context.

Show a warning or disable a tier when live RAM/disk admission fails. Do not
describe a 4 GB machine as supported. Experimental 6–8 GB Lite use must be
clearly marked and based on available-memory admission.

The download view shows source, license, total size, downloaded size, speed,
estimated time, checksum stage, pause/cancel, and recovery instructions.

### Remote choice

Collect base URL, optional key, and selected upstream model. Normalize a missing
`/v1` safely. Show separate results for model listing and minimal generation.
Save the non-secret profile; save the key only in the OS credential store.

## Target main chat layout

- Scrollable Markdown transcript with distinct user, Dociler, error, and tool
  status treatments.
- Multiline composer with `@file` autocomplete and slash-command completion.
- Compact status bar: workspace, Local/Remote, alias/model, context usage,
  read/write state, and LAN API state.
- Inline progress for scanning, parsing, indexing, map/reduce passes, and model
  generation.
- Escape cancels the active completion; Ctrl+C first cancels work and only exits
  when idle or confirmed.

Natural-language requests may search the workspace, but explicit `@file`
references take precedence. Before automatically including many documents, show
the selected set and estimated processing work.

## Slash commands

| Command | Behavior |
| --- | --- |
| `/help` | Show task examples, keys, commands, and privacy explanations. |
| `/model` | Inspect, download, switch, or unload local profiles. |
| `/connect` | List/switch profiles; `refresh` reloads disk state; `add`, `check NAME`, and `edit NAME` verify connectivity; `remove NAME` requires exact repetition; `key NAME` verifies a replacement or keyless access. |
| `/files` | Browse supported workspace documents and extraction status. |
| `/status` | Show backend health, memory/context, permissions, and server state. |
| `/clear` | Clear the in-memory transcript and extracted context after confirmation. |
| `/permissions` | Inspect or enable/disable the current workspace write grant. |
| `/export` | Export the latest answer/draft/transcript to an approved new path. |
| `/turn-on-remote` | Start the authenticated LAN API on port 11435. |
| `/turn-off-remote` | Stop and unregister the LAN API service. |
| `/update` | Check signed metadata and install only after confirmation. |
| `/exit` | Exit, asking what to do with a running LAN service. |

Only `/help`, `/status`, `/connect [NAME|add|refresh|check NAME|edit NAME|remove
NAME|key NAME]`, confirmed `/clear`, and `/exit` are active in the current
terminal surface. `dociler model status`, `model list`, and `model verify
[PROFILE]` are available outside the TUI, as is the explicitly confirmed
`model download PROFILE --confirm [--restart]`; the interactive `/model`
workflow and the remaining rows are the approved target contract. The core
download transport is cooperatively cancellable, while signal/keyboard wiring
for this scriptable command remains a later CLI/TUI integration unit; abrupt
termination safely leaves the `.partial` file resumable.

## Write experience

Enabling writes displays the canonical workspace path and explains that the
grant persists for this workspace. A mutation still follows this sequence:

1. Describe the intended result and target.
2. Show a line diff for MD/TXT or an outline/format preview for a new export.
3. Require explicit confirmation containing the target path.
4. Write to a same-directory temporary file and atomically replace or rename.
5. Retain the original text in memory for session undo; do not create hidden
   persistent document backups.

Refuse in-place binary edits even with permission. Suggest a new DOCX/RTF/ODT or
text output path instead.

## LAN server lifecycle

`/turn-on-remote` shows the interface address, port, alias, bearer-token setup,
and a warning that Dociler does not configure the firewall/router or TLS.

On TUI exit with a running server, ask:

- Keep Dociler API running in the background.
- Stop API and unload the model.
- Cancel exit.

Background mode uses launchd, systemd user services, or a Windows per-user
service adapter. `/turn-off-remote` must work from either a new TUI or a
non-interactive subcommand.

## Errors

Errors include a short outcome, likely cause, and next action. Important cases:

- scanned PDF: “No embedded text found; OCR is not included in v1”;
- encrypted document: request an unprotected copy without soliciting/storing the
  password;
- memory admission failure: offer Lite, context reduction, unload, or Remote;
- remote TLS/auth/model error: identify which verification step failed;
- parser limit: identify the affected file and configured limit;
- invalid citation: mark it unverified instead of presenting it as grounded.
