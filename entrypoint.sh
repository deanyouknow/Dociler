#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# entrypoint.sh — Startup orchestration for Document Compiler LLM (llama.cpp)
#
# Responsibilities:
#   1. Ensure base GGUF model (Qwen2.5-3B-Instruct) is downloaded in /root/models
#   2. Detect hardware (GPU / CPU)
#   3. Auto-discover skills/*.md files for FastAPI gateway
#   4. Start llama-server in the background on 127.0.0.1:8080
#   5. Wait for llama-server API to be ready
#   6. Start FastAPI gateway on 0.0.0.0:11434
# ═══════════════════════════════════════════════════════════════════════════
set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────────────
MODEL_NAME="${MODEL_NAME:-doc-compiler}"
MODEL_FILE="${MODEL_FILE:-qwen2.5-3b-instruct-q4_k_m.gguf}"
MODEL_URL="${MODEL_URL:-https://huggingface.org/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf}"
MODELS_DIR="${MODELS_DIR:-/root/models}"
SKILLS_DIR="${SKILLS_DIR:-/root/skills}"
LLAMA_CTX_SIZE="${LLAMA_CTX_SIZE:-8192}"
LLAMA_N_GPU_LAYERS="${LLAMA_N_GPU_LAYERS:-99}"
API_INTERNAL_URL="http://127.0.0.1:8080"
MAX_WAIT_SECS=120

# ── Helpers ──────────────────────────────────────────────────────────────────
log()  { echo "[$(date '+%H:%M:%S')] $*"; }
ok()   { echo "[$(date '+%H:%M:%S')] ✅ $*"; }
warn() { echo "[$(date '+%H:%M:%S')] ⚠️  $*"; }
err()  { echo "[$(date '+%H:%M:%S')] ❌ $*" >&2; }

# ── Banner ───────────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════╗"
echo "║     Document Compiler LLM (llama.cpp) v2.0   ║"
echo "╚══════════════════════════════════════════════╝"
echo ""

mkdir -p "$MODELS_DIR"

# ── 1. Ensure GGUF model is downloaded ────────────────────────────────────────
MODEL_PATH="${MODELS_DIR}/${MODEL_FILE}"
log "Checking model file: ${MODEL_PATH}..."

if [ -f "$MODEL_PATH" ] && [ -s "$MODEL_PATH" ]; then
    ok "Model file '${MODEL_FILE}' found cached in ${MODELS_DIR}"
else
    log "Model file not found. Downloading '${MODEL_FILE}' (~2.0 GB)..."
    log "Source: ${MODEL_URL}"
    if curl -L -o "$MODEL_PATH" "$MODEL_URL"; then
        ok "Model downloaded successfully to ${MODEL_PATH}"
    else
        err "Failed to download model file. Check internet connection."
        rm -f "$MODEL_PATH"
        exit 1
    fi
fi

# ── 2. Detect hardware mode ───────────────────────────────────────────────────
if command -v nvidia-smi > /dev/null 2>&1 && nvidia-smi > /dev/null 2>&1; then
    GPU_INFO=$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -1)
    ok "GPU detected: ${GPU_INFO} — CUDA offloading enabled (-ngl ${LLAMA_N_GPU_LAYERS})"
else
    warn "No NVIDIA GPU detected — running in CPU mode"
    LLAMA_N_GPU_LAYERS=0
fi

# ── 3. Auto-discover skills ───────────────────────────────────────────────────
log "Discovering skills from: ${SKILLS_DIR}"
SKILLS_COUNT=0
if [ -d "$SKILLS_DIR" ]; then
    if [ -f "${SKILLS_DIR}/skills.md" ]; then
        SKILLS_COUNT=$((SKILLS_COUNT + 1))
        log "  ✓ skills.md"
    fi
    while IFS= read -r -d '' skill_file; do
        FILENAME=$(basename "$skill_file")
        if [ "$FILENAME" != "skills.md" ]; then
            SKILLS_COUNT=$((SKILLS_COUNT + 1))
            log "  ✓ ${FILENAME}"
        fi
    done < <(find "${SKILLS_DIR}" -maxdepth 1 \( -name '*skills*.md' -o -name 'skills-*.md' \) -print0 2>/dev/null | sort -z)
fi
ok "Discovered ${SKILLS_COUNT} skill file(s) for prompt injection"

# ── 4. Verify llama-server binary ─────────────────────────────────────────────
if ! command -v llama-server > /dev/null 2>&1 && [ ! -x /usr/local/bin/llama-server ]; then
    err "llama-server binary not found!"
    exit 1
fi
LLAMA_BIN=$(command -v llama-server || echo "/usr/local/bin/llama-server")

# ── 5. Start llama-server in background ───────────────────────────────────────
log "Starting llama-server on internal port 8080..."
"$LLAMA_BIN" \
    --model "$MODEL_PATH" \
    --host 127.0.0.1 \
    --port 8080 \
    --ctx-size "$LLAMA_CTX_SIZE" \
    --n-gpu-layers "$LLAMA_N_GPU_LAYERS" \
    --alias "$MODEL_NAME" &
LLAMA_PID=$!

# ── 6. Wait for llama-server API to be ready ─────────────────────────────────
log "Waiting for llama-server API (max ${MAX_WAIT_SECS}s)..."
ELAPSED=0
until curl -sf "${API_INTERNAL_URL}/health" > /dev/null 2>&1 || curl -sf "${API_INTERNAL_URL}/v1/models" > /dev/null 2>&1; do
    sleep 2
    ELAPSED=$((ELAPSED + 2))
    if [ "$ELAPSED" -ge "$MAX_WAIT_SECS" ]; then
        err "llama-server did not become ready within ${MAX_WAIT_SECS} seconds."
        exit 1
    fi
done
ok "llama-server API is ready (took ${ELAPSED}s)"

# ── 7. Start FastAPI Gateway ──────────────────────────────────────────────────
log "Starting FastAPI Gateway on port 11434..."
export LLAMA_BACKEND="http://127.0.0.1:8080"
export SKILLS_DIR="$SKILLS_DIR"
python3 -m uvicorn --app-dir /root gateway:app --host 0.0.0.0 --port 11434 &
GATEWAY_PID=$!

# ── Ready Banner ──────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════════════╗"
echo "║  🚀 Document Compiler LLM (llama.cpp) is ready!     ║"
echo "╠══════════════════════════════════════════════════════╣"
printf  "║  %-52s ║\n" "API URL:    http://0.0.0.0:11434/v1"
printf  "║  %-52s ║\n" "Model:      ${MODEL_NAME} (skills injected)"
printf  "║  %-52s ║\n" "Base Model: qwen2.5:3b (${MODEL_FILE})"
printf  "║  %-52s ║\n" "Skills:     ${SKILLS_COUNT} file(s) loaded"
echo "╚══════════════════════════════════════════════════════╝"
echo ""

# Keep container alive by waiting on both processes
wait -n "$LLAMA_PID" "$GATEWAY_PID"
