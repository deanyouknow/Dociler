# Document Compiler LLM

A self-contained, portable LLM service optimized for document analysis and compilation. Exposes an **OpenAI-compatible REST API** at port `11434`. Under 4 GB RAM. Works on CPU or NVIDIA GPU automatically.

---

## Quick Start (New Users)

```bash
git clone <your-repo-url>
cd llm
chmod +x setup.sh
./setup.sh
```

That's it. The script handles everything:
- Checks Docker is installed
- Builds the image (or pulls from registry)
- Downloads the model (~2.5 GB, first run only)
- Waits until the API is ready

---

## Requirements

| Requirement | Minimum | Recommended |
|---|---|---|
| Docker | 24+ | Latest |
| RAM | 4 GB free | 6 GB |
| Disk | 4 GB free | 8 GB |
| GPU | Optional | 4 GB VRAM |
| Internet | First run only | — |

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

### Chat with Markdown/Text File Upload (cURL)

You can send `.md`, `.markdown`, or `.txt` files directly to the custom `/v1/chat/with-file` endpoint:

```bash
curl -X POST http://localhost:11434/v1/chat/with-file \
  -F "file=@/path/to/your-document.md" \
  -F "message=Summarize this document in Indonesian." \
  -F "stream=false"
```

Parameters:
- `file`: The `.md`, `.markdown`, or `.txt` file to upload (required).
- `message`: Your prompt/question about the document (required).
- `stream`: Set to `true` to stream the response (default: `false`).
- `temperature`: Control sampling temperature (default: `0.2`).

### List available models
```bash
curl http://localhost:11434/v1/models | jq .
```

---

## Skills System

Skills are markdown files that define the assistant's behavior and capabilities. They are injected into the model's system prompt at container startup.

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
2. Any file matching `skills-*.md` is loaded alphabetically after
3. All files are concatenated with `---` separators
4. The combined content becomes the model's `SYSTEM` prompt

### Updating Skills
Skills are bind-mounted into the container. You can update them without rebuilding the image:

```bash
# Edit your skills
nano skills/skills.md

# Restart the container to apply changes (takes ~30 seconds)
docker compose restart
```

### Writing Your Own Skills
Skills files are plain Markdown. Use headers (`##`) to organize sections. The model reads them as instructions.

Example `skills/skills-custom.md`:
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
| `MODEL_BASE` | `qwen3:4b` | Base model to pull from Ollama |
| `MODEL_NAME` | `doc-compiler` | Name of the custom model created |
| `SKILLS_DIR` | `/root/skills` | Directory to scan for skill files |
| `OLLAMA_NUM_PARALLEL` | `1` | Concurrent requests (keep at 1 to save RAM) |
| `OLLAMA_FLASH_ATTENTION` | `1` | Enables flash attention (reduces KV cache RAM) |

---

## Container Management

```bash
# Start the service
docker compose up -d

# Stop the service
docker compose stop

# Restart (re-applies skills changes)
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
| OS overhead | ~200 MB |
| **Peak total** | **~2.33 GB** ✅ |

The container is hard-capped at 4 GB via `mem_limit` in `docker-compose.yml`.

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

**GPU acceleration (NVIDIA):**
- Install `nvidia-container-toolkit` on the host
- Run: `docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d`
- Or use `./setup.sh` which auto-detects and applies the GPU override

---

## Project Structure

```
llm/
├── Dockerfile               # Container image definition
├── docker-compose.yml       # Service orchestration
├── Modelfile.template       # Model config with <<<SKILLS>>> placeholder
├── entrypoint.sh            # Startup + skills injection script
├── healthcheck.sh           # Docker health probe
├── setup.sh                 # One-time setup for new users
├── skills/
│   └── skills.md            # Default document compilation skills
├── .dockerignore            # Keeps Docker build context lean
└── README.md                # This file
```
