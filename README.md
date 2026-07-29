# Document Compiler LLM (llama.cpp)

A self-contained, **llama.cpp-based** LLM service for document analysis and compliance screening. Ships as a single Docker container running `llama-server` and a **FastAPI gateway** that exposes an **OpenAI-compatible REST API** plus a custom file-upload endpoint. Configured as an **AML (Anti-Money Laundering) screening assistant** via a pluggable skills system.

> **Branch:** `llamacpp-based` · **Container Name:** `doc-compiler-llama` · **Host Port:** `11435`

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
- Downloads the GGUF base model (`qwen2.5-3b-instruct-q4_k_m.gguf`, ~2.0 GB, first run only) into dedicated volume `doc-compiler-llama-models`
- Starts container `doc-compiler-llama` listening on host port `11435`
- Waits until the API is healthy and ready

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│              Docker Container (doc-compiler-llama)       │
│                                                          │
│  ┌──────────────┐         ┌────────────────────────┐     │
│  │  FastAPI      │:11434   │  llama-server          │     │
│  │  Gateway      │────────▶│  (internal :8080)      │     │
│  │              │         │                        │     │
│  │  /v1/*       │ proxy   │  qwen2.5:3b GGUF       │     │
│  │  /v1/chat/   │ + skills│  (qwen2.5-3b-instruct- │     │
│  │  with-file   │ inject  │   q4_k_m.gguf)         │     │
│  └──────────────┘         └────────────────────────┘     │
│         │                                                │
│         └─ Exposed Host Port: 11435                       │
│                                                          │
│  entrypoint.sh                                           │
│  ├─ Downloads GGUF model to /root/models (if missing)    │
│  ├─ Auto-discovers skills/*.md                           │
│  ├─ Starts llama-server on internal :8080                │
│  └─ Starts FastAPI gateway on :11434 (mapped to 11435)   │
└──────────────────────────────────────────────────────────┘
```

### Skills System & Model Aliasing

- Requesting **`doc-compiler`**: The gateway automatically injects the concatenated `skills.md` system prompt into the request.
- Requesting **`qwen2.5:3b`**: Accesses the raw base model without skills injection.
- Endpoint **`/v1/models`**: Reports both `doc-compiler` and `qwen2.5:3b`.

---

## Coexistence with Ollama Build

This `llama.cpp`-based build uses separate container, image, port, and volume names:
- **Container Name:** `doc-compiler-llama` (Ollama build used `doc-compiler-llm`)
- **Host Port:** `11435` (Ollama build used `11434`)
- **Volume Name:** `doc-compiler-llama-models` (Ollama build used `doc-compiler-models`)

You can run both the Ollama build and this llama.cpp build simultaneously on the same host without port or volume conflicts.

---

## Requirements

| Requirement | Minimum | Recommended |
|---|---|---|
| Docker | 24+ | Latest |
| RAM | 4 GB free | 6 GB |
| Disk | 4 GB free | 8 GB |
| GPU | Optional | NVIDIA GPU / Apple Silicon |
| Internet | First run only | — |

---

## API Usage

The API is fully compatible with the OpenAI SDK and any tool that accepts a custom base URL.

**Connection settings:**
```
Base URL:  http://localhost:11435/v1
API Key:   local   (any non-empty string works)
Model:     doc-compiler  (or qwen2.5:3b)
```

### Chat (cURL)
```bash
curl http://localhost:11435/v1/chat/completions \
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
curl http://localhost:11435/v1/chat/completions \
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
    base_url="http://localhost:11435/v1",
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
curl -X POST http://localhost:11435/v1/chat/with-file \
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
curl http://localhost:11435/v1/models | jq .
```

---

## Skills System

Skills are markdown files that define the assistant's behavior and domain expertise. They are injected into the model's system prompt at request time.

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
2. Any file matching `*skills*.md` or `skills-*.md` is loaded alphabetically after
3. All files are concatenated with `---` separators
4. The gateway prepends/injects this combined system prompt for requests targeting `doc-compiler`.

---

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `MODEL_NAME` | `doc-compiler` | Exposed model alias name |
| `MODEL_FILE` | `qwen2.5-3b-instruct-q4_k_m.gguf` | GGUF model filename |
| `LLAMA_CTX_SIZE` | `8192` | Active context window size |
| `LLAMA_N_GPU_LAYERS` | `99` | Number of layers to offload to GPU (0 = CPU only) |
| `SKILLS_DIR` | `/root/skills` | Directory containing skill markdown files |

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
docker stats doc-compiler-llama

# Rebuild image
docker compose build --no-cache

# Reset model storage
docker compose down -v
```

---

## Project Structure

```
llm/
├── Dockerfile               # Container image definition (llama-server + Gateway)
├── docker-compose.yml       # Service orchestration (doc-compiler-llama, port 11435)
├── docker-compose.gpu.yml   # GPU override for NVIDIA
├── gateway.py               # FastAPI proxy + skills injection + file upload endpoint
├── entrypoint.sh            # Startup orchestration (model download + llama-server + gateway)
├── healthcheck.sh           # Docker health probe
├── setup.sh                 # One-time setup script
├── skills/
│   └── skills.md            # Default AML screening skills
├── .gitlab-ci.yml           # GitLab CI
├── .dockerignore            # Lean Docker build context
└── README.md                # This file
```

---

## License

_No license specified yet._
