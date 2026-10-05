# Document Processing Pipeline

## Supported formats

| Format | Input | Direct edit | New export | Notes |
| --- | --- | --- | --- | --- |
| Markdown `.md` | yes | yes | yes | Preserve headings, lists, tables, links, and code fences. |
| Text `.txt` | yes | yes | yes | Detect supported encodings; normalize internally to UTF-8. |
| PDF `.pdf` | text PDFs | no | no | Preserve page anchors; image-only and encrypted PDFs are rejected. |
| Word `.docx` | yes | no | yes | Preserve structural content, not pixel-perfect layout. |
| Word `.doc` | yes | no | no | Legacy input only; suggest DOCX for output. |
| Rich Text `.rtf` | yes | no | yes | Preserve semantic text/basic formatting where available. |
| OpenDocument `.odt` | yes | no | yes | Preserve semantic text/basic structure where available. |

## Extraction boundary

Wrap Extractous behind a narrow, versioned interface. Never expose its types to
the rest of the application. Each parse runs in a new worker process with:

- read access only to the selected file;
- no network;
- a maximum input size, elapsed time, resident memory, decompressed expansion,
  extracted text, and metadata count;
- stdout reserved for a framed, versioned result protocol;
- stderr captured, sanitized, and bounded;
- forced termination on timeout, cancellation, or parent exit.

Validate extension, MIME signature, canonical path, and workspace permission
before launching the worker. Do not accept an archive's claimed file type at
face value.

## Canonical document model

Normalize every parser result into a Dociler-owned AST:

```text
Document
  metadata: title, author, created/modified when available
  source: display name, content digest, format
  blocks[]
    Heading(level, runs, anchor)
    Paragraph(runs, anchor)
    List(ordered, items, anchor)
    Table(rows, cells, anchor)
    Code(language, text, anchor)
    PageBreak(page)
    Unsupported(description, anchor)
```

Inline runs support text, emphasis, strong emphasis, and links. Anchors contain
only validated page, section, block, and chunk identities. Layout coordinates,
embedded scripts/macros, external resources, and executable content are ignored.

## Workspace discovery

- Start from the canonical current working directory or an explicit path passed
  to `dociler files [PATH]` or `/files`.
- Respect Git ignore rules when present, evaluating nested `.gitignore` files
  and handling negation patterns (`!pattern`).
- Exclude `.git`, hidden directories, dependency/build/vendor directories (`target`,
  `node_modules`, `dist`, `build`, `.cache`, `vendor`, `.gemini`), model
  caches, and Dociler output/cache names by default.
- Do not follow directory symlinks. A selected file symlink is accepted only if
  its resolved target remains strictly inside the workspace boundary.
- Inspect only supported extensions during the initial metadata scan (`.md`,
  `.txt`, `.pdf`, `.docx`, `.doc`, `.rtf`, `.odt`).
- Enforce size limits during scanning: individual files exceeding 50 MiB are
  skipped and reported in discovery summary; total document aggregate bytes
  exceeding 250 MiB abort with `TotalSizeLimitExceeded`.
- Read document contents only when explicitly selected or required by a user
  question; scanning and listing documents never loads body text or indexes
  chunks into memory.

## Chunking and indexing

Use the active model tokenizer. Split at semantic block boundaries, carrying the
nearest heading and page identity. Split an oversized table by rows while
repeating headers; split an oversized paragraph with a small token overlap.

Every chunk has a stable session-local ID, document digest, source anchor,
ordinal, token count, and normalized searchable text. Build an in-memory BM25
index. Do not add an embedding model or persist a database in v1.

## Context strategies

### Direct

For a small explicit selection, include complete source-labelled blocks within
the request budget.

### Retrieval

For a factual question, search the selected files or workspace, diversify the
highest-scoring chunks across files/sections, and include adjacent chunks where
needed for continuity. Explicit `@file` references constrain the search scope.

### Map/reduce

For whole-document summary, comparison, or compilation, process every relevant
chunk in deterministic order. Map to a compact structured record containing
claims and source IDs, then reduce records in bounded batches. Do not mistake
retrieval-only coverage for a complete summary.

### Conversation compaction

Keep recent turns verbatim and compact older conversation into an in-memory
summary without silently dropping active file selections, user constraints, or
verified source references.

## Skills, prompts, and citations

Prompt order is:

1. Dociler core safety and grounding policy.
2. Relevant immutable built-in skill modules.
3. Client/system preferences that do not conflict with Dociler policy.
4. Source-labelled document data inside explicit untrusted-content delimiters.
5. Conversation and current user request.

Require answers to cite provided source IDs for document-derived factual claims.
After generation, parse every source ID and confirm it was supplied in the
request. Remove impossible references or display them as unverified; never map
an invented identifier to a nearby real source.

## Editing and exports

The model produces a canonical Markdown draft or structured edit proposal. The
host, not the model, applies it.

- MD/TXT edits use normalized paths, diff preview, confirmation, a same-directory
  temporary file, fsync where supported, and atomic replacement.
- Session undo keeps the pre-edit bytes in memory and disappears on exit.
- DOCX/RTF/ODT exports render the canonical AST into a new file with headings,
  paragraphs, lists, tables, emphasis, and links.
- Unsupported source formatting is flattened and disclosed in the preview.
- Refuse an export path that resolves outside the granted workspace unless the
  user explicitly selects and confirms it through the platform path prompt.
- Refuse binary overwrite regardless of write permissions.

## Failure cases

- No embedded PDF text: identify likely scanned content and state OCR is deferred.
- Encrypted file: report unsupported protection; do not request/store passwords.
- Partial/corrupt extraction: do not send partial text as if complete; identify
  the file and parser result.
- Limit exceeded: name the specific limit and safe remediation.
- Unsupported encoding: preserve the original and report attempted detection.
- Cancellation: terminate extraction/generation, remove temporary data, and keep
  the interactive session responsive.
