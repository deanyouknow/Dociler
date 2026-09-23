# Models and Inference Runtime

## Runtime choice

Use `llama.cpp`, not Ollama, as the managed local runtime. It provides native
CPU/GPU execution, GGUF quantization, broad hardware backends, and an
OpenAI-compatible server while allowing Dociler to control context, concurrency,
ports, authentication, and lifecycle directly.

Initial runtime pin:

- Project: `ggml-org/llama.cpp`
- Release: `v0.4.0`
- Binary build: `b10809`
- Commit: `5266f24da75dc449bd56cbed7addb9c8e4a6a73e`

The semantic release points to the `b10809` binary build. Manifest version
`dociler-assets-v1` pins these initial archives from the official
[`b10809` release](https://github.com/ggml-org/llama.cpp/releases/tag/b10809):

| Target | Backend | Archive | Bytes | SHA-256 |
| --- | --- | --- | ---: | --- |
| macOS arm64 | Metal | `llama-b10809-bin-macos-arm64.tar.gz` | 11,123,196 | `7d692df9e1e386e62f1c12b843903218041e6cd74c9415aa39a7ed3176f9eaa2` |
| macOS x86-64 | Metal | `llama-b10809-bin-macos-x64.tar.gz` | 11,175,330 | `13b34aa8a5d87341a21065a83f54a8167e1aaa6fe0d66065de01632a1ed64be6` |
| Linux arm64 | CPU | `llama-b10809-bin-ubuntu-arm64.tar.gz` | 13,380,118 | `f2b7333971e1b7b42e9268bfdbfa30f5f56e2897156084d2251385df94aec358` |
| Linux x86-64 | CPU | `llama-b10809-bin-ubuntu-x64.tar.gz` | 16,734,586 | `5e34434ddc6d03cd1584f403201aff0d4bd1a5793a72ff7e286532dfd1e4b941` |
| Windows x86-64 | CPU | `llama-b10809-bin-win-cpu-x64.zip` | 18,407,457 | `9df3158ed228a641a4b127942d7f459f24c9e13f04682659d05c00c80099b6b5` |

Before runtime execution is enabled, the release manifest must additionally
list extracted executable/shared-library inventory and supported CPU
instructions. Never resolve `latest` during installation. Runtime upgrades
require the full platform, model, memory, and API regression suite.
The confirmed, short-lived diagnostic probe below is not release-enabled local
inference; the compiled inventory and CPU-instruction release gate remains open.

Archive metadata, read-only cache verification, bounded extraction, installed
file inventory, and a model-free runtime lifecycle probe are implemented. Model
loading/generation, accelerator qualification, and GPU packages beyond macOS
Metal remain pending.

## Primary model profiles

### Dociler Lite

- Public alias: `dociler-lite`
- Base: Qwen3.5 4B
- Quantization: Q4_K_M
- Artifact repository: `bartowski/Qwen_Qwen3.5-4B-GGUF`
- Pinned repository revision: `ba06320255db2dbec194dad738d066be90dabf29`
- Filename: `Qwen_Qwen3.5-4B-Q4_K_M.gguf`
- Exact size: 3,013,027,808 bytes (3.01 GB)
- SHA-256: `13c16f426047e2de38cd075bdade4a7bcbc8c774384876f677740cda65f8a983`
- Default active context: 8,192 tokens
- Default maximum output: 2,048 tokens
- Supported target: 8 GB total RAM
- Experimental admission: 6–8 GB only when the live memory check passes
- Predetermined fallback: Qwen3 4B Instruct 2507 Q4_K_M

### Dociler Pro

- Public alias: `dociler-pro`
- Base: Qwen3.5 9B
- Quantization: Q4_K_M
- Artifact repository: `bartowski/Qwen_Qwen3.5-9B-GGUF`
- Pinned repository revision: `2dcd842c59ea5eb119267064550a7a4c592b16c3`
- Filename: `Qwen_Qwen3.5-9B-Q4_K_M.gguf`
- Exact size: 6,169,341,984 bytes (6.17 GB)
- SHA-256: `d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43`
- Default active context: 16,384 tokens
- Default maximum output: 4,096 tokens
- Supported target: 16 GB total RAM
- Predetermined fallback: Qwen3 8B Q4_K_M

Qwen3.5 is Apache-2.0, multilingual, and natively long-context. Dociler
deliberately uses shorter working contexts to meet its memory targets and relies
on retrieval/map-reduce for large documents. Do not download or enable the
vision projector in the word-processing-only v1.

## Model identity

The aliases describe a model plus versioned Dociler skill/prompt/runtime profile.
They are not fine-tunes or newly trained models.

- `/v1/models` returns only the active alias.
- Chat responses report the requested Dociler alias.
- `/model info`, README notices, and distribution metadata disclose the exact
  upstream model, quantizer, license, hash, and runtime.
- The GGUF remains inspectable by the machine owner. Dociler provides API
  isolation, not DRM or deceptive relabelling.

## Hardware detection

Collect total and available RAM, architecture, logical/physical CPU count,
instruction support, free disk, and available accelerators without sending the
data anywhere.

Backend preference:

1. Metal on supported macOS hardware.
2. Validated CUDA on supported NVIDIA systems.
3. Validated Vulkan/other packaged acceleration when its platform matrix passes.
4. CPU fallback using the safest matching instruction build.

Do not select a backend solely because a driver/library is present. Run a short
load and generation probe before persisting it as the preferred backend.

The first M4 foundation implements a read-only subset through `dociler model
status` and `dociler doctor`. It reports host or lower Linux cgroup RAM limits,
currently available memory, logical CPU availability, physical cores when the
OS reports them, relevant runtime CPU instruction flags, the free filesystem
space for Dociler's future asset directory, and a thread recommendation that
leaves one logical CPU responsive where possible. Inspection does not enumerate
processes, create the asset directory, persist hardware data, or make a network
request.

The implementation pins `sysinfo` 0.33.1 with only its system and disk features.
On macOS it reports Metal as an unverified candidate. CUDA and Vulkan discovery
remain unimplemented, and no accelerator becomes selected until a packaged
runtime load/generation probe passes.

## Admission and runtime configuration

Before load, account for model mapping/residency, runtime allocations, context
state, batch buffers, extraction/UI reserve, current available memory, and GPU
offload duplication. Refuse or reduce context before relying on swap.

The current preflight is deliberately conservative and is not final admission:

- Hardware RAM classes use decimal manufacturer units: Lite is supported at
  8 GB and experimental from 6 GB to below 8 GB; Pro requires 16 GB.
- Current available-memory floors use the qualification RSS limits: 5.5 GiB for
  Lite and 11.5 GiB for Pro. Falling below a floor recommends closing workloads
  or using Remote mode rather than relying on swap.
- Provisional free-disk floors are the documented approximate model size plus a
  2 GiB runtime/staging reserve: 5.16 GB for Lite and 8.32 GB for Pro. Exact
  signed-manifest byte sizes will replace these planning floors before download.
- A positive result is only “ready for runtime probe.” It does not persist a
  backend choice or claim that memory, acceleration, model quality, or the
  packaged runtime has passed release qualification.
- Missing memory or disk inventory fails closed. Below 6 GB total RAM, local
  mode remains unsupported.

Defaults:

- one loaded model;
- one inference slot;
- automatic CPU threads with at least one core left responsive where possible;
- validated flash attention;
- automatic safe GPU-layer offload;
- reasoning/thinking output disabled for normal document work;
- no unbounded context shifting;
- bounded prompt and generation tokens;
- idle unload configurable, with explicit behavior for background API mode.

## Download and storage

Model/runtime files live outside the workspace in an OS-standard user data/cache
directory. Download to a restrictive partial file, support HTTP range resume,
verify declared size and SHA-256, then publish atomically without replacing an
existing final path. Never execute a runtime whose manifest signature or
checksum fails.

The current built-in manifest is authenticated as part of the Dociler binary;
there is no separately fetched manifest or update channel yet. Model URLs use
immutable Hugging Face commit revisions, and runtime URLs use the exact
`b10809` tag rather than mutable branches or `latest`. `dociler model list`
performs metadata-only presence checks. `dociler model verify [PROFILE]` reads
the selected model(s) and current-platform runtime archive in bounded chunks,
checks exact bytes and SHA-256, and returns nonzero unless all requested assets
verify. It rejects symlinked cache roots/components and non-regular files. It
does not create cache paths or download, extract, delete, repair, or execute
anything.

`dociler model download PROFILE --confirm [--restart]` implements the mutation
boundary. It shows the built-in URL, exact bytes, checksum, and destination;
downloads the pinned platform runtime followed by the chosen GGUF; uses a
per-asset process lock; sends `Accept-Encoding: identity`; and permits only the
expected HTTPS Hugging Face/GitHub redirect families. Existing partial bytes are
hashed before an exact `Range` resume. A server that ignores or contradicts the
range is rejected without appending. Size and SHA-256 must match before a
same-directory, no-clobber hard-link publication; an existing valid final is
reused, while an invalid final is never overwritten. Failed, interrupted, and
checksum-invalid partials remain for inspection/resume; only explicit
`--restart` removes the managed partial. Runtime archives remain unextracted and
nothing downloaded by this command is executed.

`dociler model runtime-install --confirm` requires the current-platform archive
to pass its manifest size and SHA-256 again, then extracts it under the same
versioned runtime namespace. Tar archives must have the exact
`llama-b10809/` root; the Windows ZIP is rooted at its top level. Extraction is
limited to 256 entries, 128 MiB per file, 256 MiB total expanded output, and
512-byte portable paths. Absolute/traversal/backslash/alternate-stream and
Windows device names, duplicates, encryption, hard links, sparse/special files,
and unsupported entry types fail closed. Safe relative library symlinks in the
official Unix archives are materialized as regular files; the installed tree
contains no links. A private JSON inventory binds every output path, exact byte
size, and SHA-256 to the built-in archive identity. The expected
`llama-server`/`llama-server.exe` must be a regular inventoried file and is the
only output granted execute permission on Unix. Staging is removed on failure,
and an existing invalid runtime directory is never overwritten.

The five official archives were inspected during this implementation: all
matched the existing manifest checksums, contained 51–61 archive entries, and
expanded to roughly 26–47 MB before safe-link materialization. The Linux x86-64
archive was also installed end-to-end into an isolated temporary data directory,
producing 60 inventoried files and 69,015,606 bytes after link materialization.
The initial extraction verification did not execute it. A later isolated
Linux x86-64 smoke test ran the same pinned executable without a model and
stopped it after its health/auth/version checks passed.

`dociler model runtime-probe --confirm` is the only current runtime execution
path. It rehashes every installed file immediately before launch, creates a
private ephemeral API-key file and empty model/cache directories, runs the
inventoried `llama-server` first with `--version`, rehashes again, then runs it
in the [pinned build's no-model router mode](https://github.com/ggml-org/llama.cpp/blob/5266f24da75dc449bd56cbed7addb9c8e4a6a73e/tools/server/server.cpp) on
an OS-selected loopback port. The child environment is cleared and rebuilt with
only required local paths, removing ambient model, provider, and proxy settings.
The router is configured for one future slot/model, at most four CPU threads
while leaving one logical CPU responsive where possible, one HTTP thread,
disabled UI/slots/autoload, and restrictive CORS. The probe requires HTTP
health, a 401 from unauthenticated `/v1/models`, an empty authenticated model
list, and `/props` reporting the pinned build and model-free router state.
Output is drained into an 8 KiB in-memory diagnostic ring; raw diagnostics,
port, and key are not printed or persisted. Cancellation, timeout, or any
validation failure kills and reaps the child; success does the same. The port
reservation-to-child-bind interval remains a local TOCTOU boundary, mitigated
by the private key challenge and strict loopback response checks. This probe
does not prove model fit, generation quality, GPU acceleration, or API safety.

Display download source and license before consent. Support list, verify, remove,
repair, and update operations without deleting unrelated cached assets. Removal
and repair remain pending.

## Qualification gates

Primary and fallback candidates run the identical versioned corpus and runtime
configuration. Ship the primary if it passes all quality, memory, platform, and
API gates. Select the fixed fallback only if it passes and the primary fails.
Do not choose a different model ad hoc without recording a new architecture
decision and updating every affected document.

Required memory gates:

- Lite: an extended 8K-context workflow on an 8 GB host, process group at or
  below 5.5 GiB peak RSS, without OOM or sustained swap thrashing.
- Pro: an extended 16K-context workflow on a 16 GB host, process group at or
  below 11.5 GiB peak RSS, without OOM or sustained swap thrashing.

Below 6 GB total RAM, local use is unsupported; guide the user to Remote mode.
