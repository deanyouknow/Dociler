# Agent and Contributor Instructions

These instructions apply to every human or AI agent working in this repository.

## Required reading

Before taking action, read:

1. `README.md`
2. `IMPLEMENTATION_PLAN.md`
3. `CHECKPOINT.md`
4. The `/docs` pages relevant to the assigned task

The implementation plan is the product specification. `CHECKPOINT.md` is the
canonical source for current execution state. If they conflict, stop and record
the conflict as a blocker rather than silently choosing one.

## Scope and safety

- Do not implement work that has not been authorized by the user or recorded as
  the active checkpoint task.
- Preserve user changes and unrelated work. Inspect the working tree before
  editing and never discard changes without explicit authorization.
- Keep the application local-first and read-only by default.
- Never commit extracted documents, document caches, model files, credentials,
  API keys, chat transcripts, generated user exports, build artifacts, or logs.
- Do not weaken security, privacy, supported-memory, or model-quality gates just
  to make a milestone appear complete.
- Do not describe a Dociler model alias as newly trained or hide required model
  attribution and licensing.

## Mandatory checkpoint workflow

Every agent that makes meaningful progress must update `CHECKPOINT.md`.

### Before substantial mutation

- Confirm the active milestone and next task match the requested work.
- Add an `IN PROGRESS` activity entry with UTC timestamp, agent identifier,
  intended task, and anticipated files.
- Record any discovered dirty-worktree overlap or blocker before editing.

### During work

- Update the current-state sections when scope, architecture, decisions,
  blockers, or the next task materially changes.
- Use one activity entry per meaningful unit of work, not per shell command.
- Preserve old activity rows; the activity log is append-only. Corrections must
  be new rows referencing the earlier entry.

### Before handoff

- Change the activity outcome from `IN PROGRESS` to the actual result, or append
  a completion entry if an in-place update would obscure history.
- List every material file or subsystem changed.
- Record verification exactly as `passed`, `failed`, or `not run`, including the
  commands or checks used.
- Update completed work, work in progress, next recommended task, blockers, and
  decisions.
- Never mark a milestone complete while required implementation, tests,
  documentation, migration, or validation remains.

## Checkpoint entry format

Use this table shape in `CHECKPOINT.md`:

| UTC timestamp | Agent | Task | Changes | Files | Verification | Outcome | Next handoff |
| --- | --- | --- | --- | --- | --- | --- | --- |

Keep entries concise but specific enough that another agent can continue without
reconstructing the previous session.

## Definition of a valid handoff

A handoff is valid only when another agent can answer all of these from the
repository:

- What is the current milestone and status?
- What changed most recently and why?
- Which tests or checks passed, failed, or were not run?
- What remains incomplete?
- What is the next bounded task?
- Are any decisions or user permissions still required?
