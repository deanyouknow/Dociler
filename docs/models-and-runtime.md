# Models and Inference Runtime

## Runtime choice

Use `llama.cpp`, not Ollama, as the managed local runtime. It provides native
CPU/GPU execution, GGUF quantization, broad hardware backends, and an
OpenAI-compatible server while allowing Dociler to control context, concurrency,
ports, authentication, and lifecycle directly.

Initial runtime pin:

- Project: `ggml-org/llama.cpp`
- Release: `v0.4.0`
- Commit: `5266f24`

The release manifest must list exact platform asset URLs, byte sizes, SHA-256
hashes, required shared libraries, supported CPU instructions, and backend.
Never resolve `latest` during installation. Runtime upgrades require the full
platform, model, memory, and API regression suite.

## Primary model profiles

### Dociler Lite

- Public alias: `dociler-lite`
- Base: Qwen3.5 4B
- Quantization: Q4_K_M
- Artifact repository: `bartowski/Qwen_Qwen3.5-4B-GGUF`
- Filename: `Qwen_Qwen3.5-4B-Q4_K_M.gguf`
- Size: 3.01 GB
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
- Filename: `Qwen_Qwen3.5-9B-Q4_K_M.gguf`
- Size: 6.17 GB
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

## Admission and runtime configuration

Before load, account for model mapping/residency, runtime allocations, context
state, batch buffers, extraction/UI reserve, current available memory, and GPU
offload duplication. Refuse or reduce context before relying on swap.

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
directory. Download to a partial file, support HTTP range resume, verify declared
size and SHA-256, then rename atomically. Never execute a runtime whose manifest
signature or checksum fails.

Display download source and license before consent. Support list, verify, remove,
repair, and update operations without deleting unrelated cached assets.

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
