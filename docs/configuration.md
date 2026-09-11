# Configuration Reference

## Available commands

```sh
dociler config paths
dociler config show
dociler config init
dociler doctor
```

`paths` prints locations without creating them. `show` loads and validates
settings, then reports only the selected local profile and current workspace's
policy. `doctor` also validates settings. Missing configuration means safe
defaults in memory; neither command creates state or reads documents.

`init` explicitly creates a default config file. It does not overwrite an
existing file, even if that file is malformed. No model download, network
request, cache creation, service startup, or workspace grant is triggered.
Initialization uses a same-directory private temporary file, synchronization,
and no-clobber publication. A concurrent initializer cannot overwrite the winner.

## Locations

Paths use `directories::ProjectDirs::from("", "", "dociler")`, with local
configuration/data on Windows so workspace grants do not roam between machines.
See the [directories API reference](https://docs.rs/directories/6.0.0/directories/struct.ProjectDirs.html).

| Purpose | Linux default | macOS default | Windows default |
| --- | --- | --- | --- |
| Configuration | `~/.config/dociler/config.json` | `~/Library/Application Support/dociler/config.json` | `%LOCALAPPDATA%\dociler\config\config.json` |
| Model/runtime data | `~/.local/share/dociler/` | `~/Library/Application Support/dociler/` | `%LOCALAPPDATA%\dociler\data\` |
| Cache | `~/.cache/dociler/` | `~/Library/Caches/dociler/` | `%LOCALAPPDATA%\dociler\cache\` |

Linux respects absolute XDG directory overrides. Model/runtime locations have
`models` and `runtimes` subdirectories; these are reserved paths, not downloaded
assets. No chat or document cache is written.

Optional `DOCILER_CONFIG_DIR` selects an absolute configuration directory only.
It does not relocate models/data/cache. Empty or relative values are errors;
there is no implicit current-directory fallback. Use a dedicated user-owned
directory outside the repository. This also permits isolated subprocess tests
without touching the developer's real settings. There is no workspace config
or `.env` auto-loading.

## Schema version 1

```json
{
  "schema_version": 1,
  "preferred_local_profile": "dociler-lite",
  "write_workspaces": []
}
```

`schema_version` is required. Omitted profile and workspace list default to Lite
and no grants. The only profiles are `dociler-lite` and `dociler-pro`; choosing
one is a preference, not a model-load request or memory qualification result.

Settings are limited to 64 KiB and 256 workspace grants. Unknown fields,
duplicate fields, invalid types/profiles, relative grant paths, invalid UTF-8,
malformed JSON, and unsupported versions fail closed. No automatic migration
exists yet. Errors do not echo the rejected key/value or file contents.
The parser uses [Serde's unknown-field rejection](https://serde.rs/container-attrs.html).

Only initialization and reading are currently available. A future permissions
UI must save a grant from the canonical workspace identity after confirmation;
there is no grant/revoke command or settings-update writer yet. A stored grant
matches only the exact canonical workspace, never descendants or a symlink
alias that later points somewhere else. Its policy is **confirm every write**,
not unrestricted access. Editing/export services do not exist yet and must
implement containment, preview, and target-specific confirmation before use.

On Unix, the config directory/file must be user-only (created with 0700/0600).
Overly permissive existing settings are rejected, not silently chmodded. Config
file/directory symlinks and non-regular config files are rejected. On Windows,
new files inherit the user's local application directory ACL; restrictive ACL
behavior still needs native validation. These checks are not protection against
malicious code running as the same OS user or a full race-proof file sandbox.

## Session and credential boundaries

Session messages are memory-only, non-serializable, and excluded from debug
output. The current defensive cap is 1 MiB of UTF-8 message text and 512 messages;
overflow rejects the new message without discarding earlier ones. This is a
storage bound, not model-token budgeting. Clearing/dropping messages zeroizes
owned strings; it does not promise protection against OS swap, external copies,
or process inspection.

The credential interface accepts opaque profile IDs and redacted, zeroizing
secret values. There is no native keychain adapter yet: the unavailable adapter
returns an explicit error on get/set/delete, never a plaintext-file fallback.
Do not place API keys in this JSON file or command-line arguments. Remote
profiles, destination-bound consent, native keychain integration, and the TUI
will arrive in later milestones.
