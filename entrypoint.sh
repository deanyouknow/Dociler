#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# entrypoint.sh — Startup orchestration for the Document Compiler LLM
#
# Responsibilities:
#   1. Start ollama serve in the background
#   2. Wait for the API to be ready
#   3. Pull the base model (cached in volume, only downloads once)
#   4. Auto-discover and concatenate all skills/*.md files
#   5. Inject skills into the Modelfile template
#   6. Create the custom 'doc-compiler' model
#   7. Keep the container alive
# ═══════════════════════════════════════════════════════════════════════════
set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────────────
MODEL_BASE="${MODEL_BASE:-qwen3:4b}"
MODEL_NAME="${MODEL_NAME:-doc-compiler}"
SKILLS_DIR="${SKILLS_DIR:-/root/skills}"
MODELFILE_TEMPLATE="${MODELFILE_TEMPLATE:-/root/Modelfile.template}"
MODELFILE_RESOLVED="/tmp/Modelfile.resolved"
API_URL="http://localhost:11434"
MAX_WAIT_SECS=120

# ── Helpers ──────────────────────────────────────────────────────────────────
log()  { echo "[$(date '+%H:%M:%S')] $*"; }
ok()   { echo "[$(date '+%H:%M:%S')] ✅ $*"; }
warn() { echo "[$(date '+%H:%M:%S')] ⚠️  $*"; }
err()  { echo "[$(date '+%H:%M:%S')] ❌ $*" >&2; }

# ── Banner ───────────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════╗"
echo "║        Document Compiler LLM v1.0           ║"
echo "╚══════════════════════════════════════════════╝"
echo ""

# ── 1. Start Ollama in the background ────────────────────────────────────────
log "Starting Ollama server..."
ollama serve &
OLLAMA_PID=$!

# ── 2. Wait for Ollama API to be ready ───────────────────────────────────────
log "Waiting for Ollama API (max ${MAX_WAIT_SECS}s)..."
ELAPSED=0
until curl -sf "${API_URL}/api/tags" > /dev/null 2>&1; do
    sleep 2
    ELAPSED=$((ELAPSED + 2))
    if [ "$ELAPSED" -ge "$MAX_WAIT_SECS" ]; then
        err "Ollama API did not become ready within ${MAX_WAIT_SECS} seconds."
        exit 1
    fi
done
ok "Ollama API is ready (took ${ELAPSED}s)"

# ── 3. Detect hardware mode ───────────────────────────────────────────────────
if nvidia-smi > /dev/null 2>&1; then
    GPU_INFO=$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -1)
    ok "GPU detected: ${GPU_INFO} — CUDA acceleration enabled"
else
    warn "No NVIDIA GPU detected — running in CPU-only mode (~5-15 tok/s)"
fi

# ── 4. Pull base model ────────────────────────────────────────────────────────
log "Checking base model: ${MODEL_BASE}..."
if ollama list 2>/dev/null | grep -q "^${MODEL_BASE}"; then
    ok "Model '${MODEL_BASE}' already cached — skipping download"
else
    log "Downloading '${MODEL_BASE}' (~2.5 GB). This only happens once..."
    if ollama pull "${MODEL_BASE}"; then
        ok "Model downloaded successfully"
    else
        err "Failed to pull model '${MODEL_BASE}'. Check your internet connection."
        exit 1
    fi
fi

# ── 5. Auto-discover and concatenate all skills/*.md files ────────────────────
log "Loading skills from: ${SKILLS_DIR}"
SKILLS_CONTENT=""
SKILLS_COUNT=0

# Always load skills.md first (primary, canonical skill file)
if [ -f "${SKILLS_DIR}/skills.md" ]; then
    SKILLS_CONTENT=$(cat "${SKILLS_DIR}/skills.md")
    SKILLS_COUNT=$((SKILLS_COUNT + 1))
    log "  ✓ skills.md"
else
    warn "Primary skills.md not found in ${SKILLS_DIR}"
fi

# Load additional skills-*.md files in alphabetical order
while IFS= read -r -d '' skill_file; do
    FILENAME=$(basename "$skill_file")
    SKILLS_CONTENT="${SKILLS_CONTENT}

---

$(cat "$skill_file")"
    SKILLS_COUNT=$((SKILLS_COUNT + 1))
    log "  ✓ ${FILENAME}"
done < <(find "${SKILLS_DIR}" -maxdepth 1 -name 'skills-*.md' -print0 2>/dev/null | sort -z)

# Fallback if no skills found at all
if [ "$SKILLS_COUNT" -eq 0 ]; then
    warn "No skills files found — using minimal fallback prompt"
    SKILLS_CONTENT="You are a helpful document assistant. Help users read, understand, summarize, and compile documents. Respond in the same language the user uses."
fi

ok "Loaded ${SKILLS_COUNT} skill file(s)"

# ── 6. Build the resolved Modelfile from template ─────────────────────────────
log "Building Modelfile with injected skills..."

# Write skills content to a temp file first — this safely handles
# multiline text, special characters, quotes, and backslashes
# without needing Python or any external tools.
printf '%s' "$SKILLS_CONTENT" > /tmp/skills_content.txt

# Use awk to substitute the <<<SKILLS>>> placeholder with the file contents.
# Reading from a file (not a variable) avoids all quoting/escaping pitfalls.
awk '
/<<<SKILLS>>>/ {
    while ((getline line < "/tmp/skills_content.txt") > 0) {
        print line
    }
    close("/tmp/skills_content.txt")
    next
}
{ print }
' "$MODELFILE_TEMPLATE" > "$MODELFILE_RESOLVED"

log "   Modelfile written to ${MODELFILE_RESOLVED}"

# ── 7. Create the custom model ────────────────────────────────────────────────
log "Creating model '${MODEL_NAME}' (applying skills + parameters)..."
if ollama create "${MODEL_NAME}" -f "${MODELFILE_RESOLVED}"; then
    ok "Model '${MODEL_NAME}' created successfully"
else
    err "Failed to create model '${MODEL_NAME}'"
    exit 1
fi

# ── Ready ─────────────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════════════╗"
echo "║  🚀 Document Compiler LLM is ready!                 ║"
echo "╠══════════════════════════════════════════════════════╣"
printf  "║  %-52s ║\n" "API URL:    http://0.0.0.0:11434/v1"
printf  "║  %-52s ║\n" "Model:      ${MODEL_NAME}"
printf  "║  %-52s ║\n" "Base model: ${MODEL_BASE}"
printf  "║  %-52s ║\n" "Skills:     ${SKILLS_COUNT} file(s) loaded"
echo "╚══════════════════════════════════════════════════════╝"
echo ""

# Keep the container alive by waiting on ollama serve
wait "$OLLAMA_PID"
