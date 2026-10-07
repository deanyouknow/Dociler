# Product Requirements

## Product statement

Dociler is a local-first terminal assistant that helps general users work with
documents in the folder where it is launched. It combines deterministic local
document extraction and retrieval with either a resource-conscious local LLM or
a user-selected OpenAI-compatible remote model.

The first release is a document assistant, not a coding agent or full office
suite. Its primary value is private, grounded analysis on ordinary consumer
hardware with a CLI approachable enough for users unfamiliar with local LLMs.

## Audience

- Privacy-conscious people handling financial, legal, business, research, or
  personal documents.
- Users with 8 GB-class and 16 GB-class laptops who cannot run large models.
- Technical users who want an OpenAI-compatible local endpoint for another
  harness.
- New local-LLM users who need guided installation, model selection, and clear
  resource/error reporting.

## Goals

1. Let a user launch `dociler` in a folder and ask natural-language questions
   about supported documents in that workspace.
2. Extract, retrieve, summarize, compare, and cite document content without
   sending it off-device in local mode.
3. Provide useful local tiers on supported 8 GB and 16 GB machines.
4. Offer the same document skills when using a remote OpenAI-compatible model.
5. Expose a safe, authenticated OpenAI-compatible API when explicitly enabled.
6. Default to read-only and ephemeral behavior while allowing carefully
   confirmed text edits and new exports.

## Non-goals for v1

- Spreadsheet or structured-data analysis.
- Presentation files.
- Websites, browser automation, Google Docs, or MCP connectors.
- OCR for scanned PDFs or images.
- Persistent vector databases or an embedding-model download.
- Arbitrary shell commands, source-code modification, or a general coding agent.
- User-defined global or project skills.
- High-fidelity, in-place editing of PDF or binary office formats.
- Docker as the primary or supported interactive installation path.

## Supported inputs and outputs

Inputs: `.md`, `.pdf`, `.docx`, `.doc`, `.rtf`, `.odt`, and `.txt`.

Embedded-text PDFs are supported. Image-only/scanned and password-protected
documents must be detected and reported without fabricating content.

Direct edits are limited to `.md` and `.txt`. New exports may be Markdown, TXT,
DOCX, RTF, or ODT. PDF and legacy DOC are input-only; no binary source is ever
overwritten.

## Core user journeys

### New user, local model

1. Clone the repository and run the platform installer.
2. Run `dociler` in a document folder.
3. Review detected hardware and choose Local.
4. Select the recommended Lite profile or the higher-quality Pro profile.
5. Review download/resource details and complete a resumable download.
6. Enter chat, select files with `@`, and receive streamed, cited answers.

### New user, remote model

1. Choose Remote during onboarding.
2. Enter an OpenAI-compatible base URL, optional API key, and model.
3. Let Dociler verify model listing and minimal generation.
4. Before the first document leaves the machine, approve transmission to the
   displayed destination for the current session.
5. Chat using the same extraction, retrieval, and built-in skill behavior.

### Returning user

The landing screen offers the last successful profile, another downloaded local
profile, or a saved remote connection. It never silently starts a large model or
sends a document remotely.

### Controlled editing

The user enables write access for the current workspace through `/permissions`.
Dociler remembers that workspace grant but still previews and confirms each
write. Text edits are atomic and undoable during the session. Binary inputs can
only produce a new export path.

### LAN API

The user enters `/turn-on-remote`, receives the address and bearer-token setup,
and connects another harness. When leaving the TUI, Dociler asks whether the
service should stop or continue as a user-level background process.

## Functional requirements

- Discover supported documents without reading unrelated file types.
- Search and cite across one or many documents.
- Preserve headings, paragraphs, lists, tables, and useful page/section anchors.
- Handle documents larger than the active context through deterministic
  chunking, retrieval, and map/reduce summarization.
- Stream model output and allow cancellation.
- Show active workspace, backend, model profile, context, and resource state.
- Verify all model/runtime downloads before use.
- Support local operation without network access once assets are cached.
- Never expose upstream base-model IDs in the public model-list endpoint.
- Clearly disclose upstream models and licenses in model information and notices.
- Provide actionable diagnostics for unsupported, malformed, scanned, encrypted,
  oversized, or out-of-workspace inputs.

## Success criteria

- A new user can reach a first local or remote response without manually
  installing a model runtime.
- Every claimed input format passes golden extraction fixtures on every supported
  platform.
- Answers can trace factual claims to validated file/page/section references.
- Local profiles pass the quality and constrained-memory release gates in
  `testing-and-release.md`.
- The local workflow performs no network call after verified assets are cached.
- The application writes no user document without both a workspace grant and a
  per-operation confirmation.
- Another OpenAI-compatible client can list the Dociler alias and stream a chat
  completion through the authenticated gateway.

## Post-v1 roadmap

Features explicitly deferred from v1 that may be considered in post-v1 releases:

- **Spreadsheets and structured data:** Ingestion of `.xlsx`, `.csv`, and `.tsv` with tabular querying.
- **Presentations:** Slide deck ingestion for `.pptx` and `.odp`.
- **OCR extension pack:** Optional plugin for local OCR on image-only/scanned PDFs and scanned pages.
- **Connectors & Web:** Read-only ingestion for web URLs, Google Docs, and Model Context Protocol (MCP) servers.
- **Persistent RAG & embeddings:** Optional opt-in persistent vector indexes with user-controlled storage.
- **Custom skill definitions:** Sandboxed, declarative user/workspace skills beyond built-in modules.
- **Docker distribution:** Standard OCI container image for headless server deployments.
