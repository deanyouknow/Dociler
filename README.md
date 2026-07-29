# Document Compiler LLM

A self-contained, Ollama-based LLM service for document analysis and compliance screening. Ships as a single Docker container with a **FastAPI gateway** that exposes an **OpenAI-compatible REST API** plus a custom file-upload endpoint. Currently configured as an **AML (Anti-Money Laundering) screening assistant** via a pluggable skills system.

> **Branch:** `ollama-based` · **Tag:** `stable-ollama`

---

## Quick Start

```bash
git clone <your-repo-url>
cd llm
chmod +x setup.sh
./setup.sh
```

The setup script handles everything automatically:
- Checks Docker & Docker Compose are installed
- Detects GPU (NVIDIA or Apple Silicon) and applies the correct config
- Builds the Docker image
- Downloads the base model (~2.0 GB, first run only)
- Waits until the API is healthy and ready

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                Docker Container                      │
│                                                      │
│  ┌──────────────┐       ┌──────────────────────┐     │
│  │  FastAPI      │:11434 │  Ollama Server       │     │
│  │  Gateway      │──────▶│  (internal :11435)   │     │
│  │              │       │                      │     │
│  │  /v1/*       │ proxy │  qwen2.5:3b base     │     │
│  │  /v1/chat/   │       │  doc-compiler model  │     │
│  │  with-file   │       │                      │     │
│  └──────────────┘       └──────────────────────┘     │
│                                                      │
│  entrypoint.sh                                       │
│  ├─ Starts Ollama on :11435                          │
│  ├─ Pulls base model (cached in volume)              │
│  ├─ Auto-discovers skills/*.md → system prompt       │
│  ├─ Creates custom 'doc-compiler' model              │
│  └─ Starts FastAPI gateway on :11434                 │
└──────────────────────────────────────────────────────┘
```

The **FastAPI gateway** ([gateway.py](file:///Users/deand/mulai/llm/gateway.py)) serves two purposes:
1. **File upload endpoint** (`/v1/chat/with-file`) — accepts `.md`, `.markdown`, and `.txt` files, injects them into the prompt, and forwards to Ollama.
2. **Catch-all proxy** — forwards all other requests (chat completions, model listing, etc.) directly to the internal Ollama server, preserving full OpenAI API compatibility.

---

## Requirements

| Requirement | Minimum | Recommended |
|---|---|---|
| Docker | 24+ | Latest |
| RAM | 4 GB free | 6 GB |
| Disk | 4 GB free | 8 GB |
| GPU | Optional | NVIDIA 4 GB VRAM / Apple Silicon |
| Internet | First run only | — |

---

## ⚠️ Known Limitations (Ollama-Based Build)

### Inference Timeout on Low-Spec Hardware

This build runs LLM inference locally via Ollama inside a Docker container. On machines with **limited compute resources** (slow CPU, low RAM, no GPU), the model may take a very long time to generate responses — especially for long documents or complex prompts.

**If the model takes longer than ~5 minutes to produce a response, Ollama will terminate the request with a timeout error.** This is an inherent limitation of the Ollama runtime, not the application itself.

**Symptoms:**
- Request hangs for several minutes, then returns a timeout / connection error
- Gateway returns `500` with a message like `Error connecting to Ollama`
- In streaming mode, the connection drops mid-response

**Who is affected:**
- Machines running CPU-only inference without AVX2 support
- Systems with less than 4 GB of free RAM during inference
- VMs or cloud instances with shared / throttled CPUs
- Very large documents (>8K tokens of input) on underpowered hardware

**Workarounds:**
- **Use a GPU** — even a modest NVIDIA GPU (4 GB VRAM) dramatically speeds up inference
- **Reduce document size** — split large documents into smaller sections before sending
- **Lower context window** — set `num_ctx` to `4096` in [Modelfile.template](file:///Users/deand/mulai/llm/Modelfile.template) to reduce memory pressure
- **Use streaming** — set `"stream": true` in your request so partial results are delivered as they are generated, keeping the connection alive
- **Increase the gateway timeout** — the gateway uses a 300-second (5 min) timeout in [gateway.py](file:///Users/deand/mulai/llm/gateway.py); you can increase the `timeout=300.0` values if needed, though Ollama itself may still cut off long-running inference

---

## API Usage

The API is fully compatible with the OpenAI SDK and any tool that accepts a custom base URL.

**Connection settings:**
```
Base URL:  http://localhost:11434/v1
API Key:   local   (any non-empty string works)
Model:     doc-compiler
```

### Chat (cURL)
```bash
curl http://localhost:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "doc-compiler",
    "stream": false,
    "messages": [
      {
        "role": "user",
        "content": "Summarize the following document:\n\n[paste document here]"
      }
    ]
  }'
```

### Streaming (cURL)
```bash
curl http://localhost:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "doc-compiler",
    "stream": true,
    "messages": [{"role": "user", "content": "Explain this contract section: ..."}]
  }'
```

### Python (OpenAI SDK)
```python
from openai import OpenAI

client = OpenAI(
    base_url="http://localhost:11434/v1",
    api_key="local"
)

response = client.chat.completions.create(
    model="doc-compiler",
    messages=[
        {"role": "user", "content": "Summarize: [your document text]"}
    ]
)

print(response.choices[0].message.content)
```

### File Upload (cURL)

Send `.md`, `.markdown`, or `.txt` files directly to the custom `/v1/chat/with-file` endpoint:

```bash
curl -X POST http://localhost:11434/v1/chat/with-file \
  -F "file=@/path/to/your-document.md" \
  -F "message=Summarize this document in Indonesian." \
  -F "stream=false"
```

Parameters:
- `file` — the `.md`, `.markdown`, or `.txt` file to upload (**required**)
- `message` — your prompt/question about the document (**required**)
- `stream` — set to `true` for streaming response (default: `false`)
- `temperature` — sampling temperature (default: `0.2`)
- `model` — model name (default: `doc-compiler`)

### List Available Models
```bash
curl http://localhost:11434/v1/models | jq .
```

---

## Skills System

Skills are markdown files that define the assistant's behavior and domain expertise. They are injected into the model's system prompt at container startup.

### Default Skill

The default skill ([skills/skills.md](file:///Users/deand/mulai/llm/skills/skills.md)) configures the model as an **AML screening assistant** that performs:
1. Transaction listing and validation
2. Structuring detection (cash deposits near reporting thresholds)
3. Layering detection (rapid wire in → wire out to different countries)
4. High-risk country matching
5. Risk scoring (LOW / MEDIUM / HIGH / CRITICAL)

### File Locations
```
skills/
├── skills.md          ← Primary skills file (always loaded first)
├── skills-legal.md    ← Additional skill (auto-discovered)
├── skills-finance.md  ← Additional skill (auto-discovered)
└── ...
```

### How Auto-Discovery Works
1. `skills.md` is always loaded first
2. Any file matching `*skills*.md` is loaded alphabetically after
3. All files are concatenated with `---` separators
4. The combined content becomes the model's `SYSTEM` prompt via the `<<<SKILLS>>>` placeholder in [Modelfile.template](file:///Users/deand/mulai/llm/Modelfile.template)

### Updating Skills

Skills are bind-mounted into the container. Update them without rebuilding:

```bash
# Edit your skills
nano skills/skills.md

# Restart to apply changes (~30 seconds)
docker compose restart
```

### Writing Custom Skills

Skills files are plain Markdown. Use headers (`##`) to organize sections:

```markdown
## Custom Document Type: Internal Reports

When processing internal reports:
- Always extract the author, date, and department
- Summarize action items in a numbered list
- Flag any mentions of budget figures
- Output a "Risk Level" field: Low / Medium / High
```

---

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `MODEL_BASE` | `qwen2.5:3b` | Base model to pull from Ollama |
| `MODEL_NAME` | `doc-compiler` | Name of the custom model created |
| `SKILLS_DIR` | `/root/skills` | Directory to scan for skill files |
| `OLLAMA_NUM_PARALLEL` | `1` | Concurrent requests (keep at 1 to save RAM) |
| `OLLAMA_MAX_LOADED_MODELS` | `1` | Max models loaded in memory simultaneously |
| `OLLAMA_FLASH_ATTENTION` | `1` | Enables flash attention (reduces KV cache RAM) |

---

## Container Management

```bash
# Start the service
docker compose up -d

# Stop the service
docker compose stop

# Restart (re-applies skill changes)
docker compose restart

# View startup logs
docker compose logs -f llm

# Monitor memory usage
docker stats doc-compiler-llm

# Rebuild image (after Dockerfile changes)
docker compose build --no-cache

# Full reset (removes model cache — will re-download)
docker compose down -v
```

---

## Memory Usage

| Component | Size |
|---|---|
| Ollama binary | ~80 MB |
| qwen2.5:3b Q4_K_M weights | ~2.0 GB |
| KV cache (8K ctx) | ~150 MB |
| FastAPI gateway | ~30 MB |
| OS overhead | ~200 MB |
| **Peak total** | **~2.5 GB** |

The container is hard-capped at 6 GB via `mem_limit` in [docker-compose.yml](file:///Users/deand/mulai/llm/docker-compose.yml).

---

## Troubleshooting

**Container exits immediately:**
```bash
docker compose logs llm
```
Usually caused by a failed model pull. Check your internet connection.

**API returns 404 or connection refused:**
```bash
docker compose ps          # Is the container running?
docker compose logs llm    # Check for errors
```

**Out of memory errors:**
- Ensure no other large applications are using RAM
- Set `OLLAMA_NUM_PARALLEL=1` (default)
- Reduce context: lower `num_ctx` in `Modelfile.template` to `4096`

**Timeout errors (see [Known Limitations](#️-known-limitations-ollama-based-build)):**
- Use streaming mode to keep the connection alive
- Reduce input document size
- Consider running on hardware with a dedicated GPU

**GPU acceleration (NVIDIA):**
- Install `nvidia-container-toolkit` on the host
- Run: `docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d`
- Or use `./setup.sh` which auto-detects and applies the GPU override

---

## Project Structure

```
llm/
├── Dockerfile               # Container image (Ollama + Python + FastAPI)
├── docker-compose.yml       # Service orchestration (CPU mode)
├── docker-compose.gpu.yml   # GPU override for NVIDIA
├── Modelfile.template       # Model config with <<<SKILLS>>> placeholder
├── gateway.py               # FastAPI proxy + file upload endpoint
├── entrypoint.sh            # Startup orchestration (Ollama + skills + gateway)
├── healthcheck.sh           # Docker health probe
├── setup.sh                 # One-time setup for new users
├── skills/
│   └── skills.md            # Default AML screening skills
├── .gitlab-ci.yml           # GitLab CI (SAST + Secret Detection)
├── .dockerignore            # Lean Docker build context
└── README.md                # This file
```

---

## CI/CD

The project includes a [.gitlab-ci.yml](file:///Users/deand/mulai/llm/.gitlab-ci.yml) pipeline with:
- **SAST** (Static Application Security Testing)
- **Secret Detection** (scans for leaked credentials)

---

## License

_No license specified yet._
