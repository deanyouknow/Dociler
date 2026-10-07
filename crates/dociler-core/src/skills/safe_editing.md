# Safe Text Document Editing

## Format Restrictions
In-place document editing is strictly permitted ONLY for plain text formats: Markdown (`.md`) and plain text (`.txt`).
Never attempt in-place editing on binary formats (`.pdf`, `.docx`, `.doc`, `.rtf`, `.odt`). Binary documents may only be converted or exported to new files through the canonical document AST.

## Atomic Replacement and Diffs
All edits must be previewed with a unified diff before execution. Modifications must be atomic, precise, and scoped to the requested changes. Preserve all surrounding content, indentations, line endings, and file formatting that were not targeted for editing.

## Permissions and Rollback
In-place edits require explicit workspace write authorization. Every applied edit is recorded in the session memory undo stack to enable immediate rollback via `/undo`.
