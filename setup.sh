#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# setup.sh — One-time setup for new users
#
# Usage:
#   ./setup.sh            # Standard setup
#   ./setup.sh --rebuild  # Force a clean rebuild of the Docker image
# ═══════════════════════════════════════════════════════════════════════════
set -euo pipefail

REBUILD=false
USE_GPU=false
COMPOSE_FILES="-f docker-compose.yml"

if [[ "${1:-}" == "--rebuild" ]]; then
    REBUILD=true
fi

API_URL="http://localhost:11434"
MAX_WAIT_SECS=500  # 5 minutes — first run downloads ~2.5 GB model

# ── Helpers ──────────────────────────────────────────────────────────────────
log()  { echo "  $*"; }
ok()   { echo "  ✅ $*"; }
warn() { echo "  ⚠️  $*"; }
err()  { echo "  ❌ $*" >&2; }
header() { echo ""; echo "── $* ──────────────────────────────────────────"; }

# ── Banner ───────────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════╗"
echo "║     Document Compiler LLM — First Setup     ║"
echo "╚══════════════════════════════════════════════╝"

# ── Check prerequisites ───────────────────────────────────────────────────────
header "Checking prerequisites"

if ! command -v docker &> /dev/null; then
    err "Docker is not installed."
    echo "     Install it from: https://docs.docker.com/get-docker/"
    exit 1
fi
ok "Docker found: $(docker --version | head -1)"

if ! docker compose version &> /dev/null; then
    err "Docker Compose v2 is not available."
    echo "     It comes bundled with Docker Desktop. Update Docker or install the plugin."
    exit 1
fi
ok "Docker Compose found: $(docker compose version --short)"

# Check if Docker daemon is running
if ! docker info &> /dev/null; then
    err "Docker daemon is not running. Please start Docker and try again."
    exit 1
fi
ok "Docker daemon is running"

# ── Check for GPU (informational only) ────────────────────────────────────────
header "Hardware detection"
if nvidia-smi &> /dev/null && docker info 2>/dev/null | grep -q "nvidia"; then
    GPU=$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -1)
    ok "NVIDIA GPU detected: ${GPU}"
    log "Applying GPU override (docker-compose.gpu.yml)..."
    USE_GPU=true
    COMPOSE_FILES="-f docker-compose.yml -f docker-compose.gpu.yml"
elif [[ "$(uname -m)" == "arm64" ]] && [[ "$(uname)" == "Darwin" ]]; then
    ok "Apple Silicon detected — Metal GPU acceleration will be used by Ollama automatically"
else
    warn "No GPU detected — running in CPU-only mode"
    log "Inference speed: ~5-15 tokens/second (adequate for document tasks)"
fi

# ── Build or pull image ───────────────────────────────────────────────────────
header "Setting up Docker image"

if [ "$REBUILD" = true ]; then
    log "Rebuilding image from scratch (--rebuild flag set)..."
    docker compose $COMPOSE_FILES build --no-cache
    ok "Image rebuilt"
else
    REGISTRY_IMAGE=$(docker compose $COMPOSE_FILES config --images 2>/dev/null | head -1 || echo "")

    if [[ "$REGISTRY_IMAGE" == registry.gitlab.com* ]]; then
        log "Attempting to pull from GitLab registry..."
        if docker compose $COMPOSE_FILES pull 2>/dev/null; then
            ok "Image pulled from registry"
        else
            warn "Registry pull failed — building locally..."
            docker compose $COMPOSE_FILES build
            ok "Image built locally"
        fi
    else
        log "Building image locally..."
        docker compose $COMPOSE_FILES build
        ok "Image built"
    fi
fi

# ── Start the service ─────────────────────────────────────────────────────────
header "Starting LLM service"
log "Starting container..."
docker compose $COMPOSE_FILES up -d
ok "Container started"

# ── Wait for model to be ready ────────────────────────────────────────────────
header "Waiting for model to be ready"
log "This may take several minutes on first run (downloading ~2.5 GB model)..."
log "You can watch progress in another terminal with: docker compose logs -f llm"
echo ""

ELAPSED=0
SPINNER=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
SPIN_IDX=0

until curl -sf "${API_URL}/v1/models" > /dev/null 2>&1; do
    printf "\r  %s  Elapsed: %ds" "${SPINNER[$SPIN_IDX]}" "$ELAPSED"
    sleep 3
    ELAPSED=$((ELAPSED + 3))
    SPIN_IDX=$(( (SPIN_IDX + 1) % ${#SPINNER[@]} ))

    if [ "$ELAPSED" -ge "$MAX_WAIT_SECS" ]; then
        echo ""
        err "Service did not become ready within ${MAX_WAIT_SECS} seconds."
        echo ""
        echo "  Troubleshooting:"
        echo "    docker compose logs llm     # See what's happening"
        echo "    docker compose ps           # Check container status"
        exit 1
    fi

    # Check if container crashed
    STATUS=$(docker compose ps --format json llm 2>/dev/null | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('State',''))" 2>/dev/null || echo "")
    if [[ "$STATUS" == "exited" ]]; then
        echo ""
        err "Container exited unexpectedly."
        echo "  Run: docker compose logs llm"
        exit 1
    fi
done

echo ""
echo ""

# ── Verify model is available ──────────────────────────────────────────────────
MODEL_LIST=$(curl -sf "${API_URL}/v1/models" | python3 -c "
import sys, json
data = json.load(sys.stdin)
models = [m['id'] for m in data.get('data', [])]
print(', '.join(models))
" 2>/dev/null || echo "unknown")

# ── Done ──────────────────────────────────────────────────────────────────────
echo "╔══════════════════════════════════════════════════════════╗"
echo "║            ✅  Setup Complete!                          ║"
echo "╠══════════════════════════════════════════════════════════╣"
printf "║  %-56s ║\n" "API URL:   http://localhost:11434/v1"
printf "║  %-56s ║\n" "Model:     doc-compiler"
printf "║  %-56s ║\n" "API Key:   any string (e.g. 'local')"
printf "║  %-56s ║\n" "Available: ${MODEL_LIST}"
echo "╠══════════════════════════════════════════════════════════╣"
echo "║  Quick test:                                            ║"
echo "║    curl http://localhost:11434/v1/chat/completions \\    ║"
echo "║      -H 'Content-Type: application/json' \\             ║"
echo '║      -d '"'"'{"model":"doc-compiler","stream":false,       ║'
echo '║           "messages":[{"role":"user",                   ║'
echo '║           "content":"Summarize: Hello World"}]}'"'"'       ║'
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "  Useful commands:"
echo "    docker compose logs -f llm      # Follow logs"
echo "    docker compose stop             # Stop the service"
echo "    docker compose start            # Start again"
echo "    docker compose restart          # Restart (reload skills)"
echo "    docker stats doc-compiler-llm   # Monitor memory usage"
echo ""
