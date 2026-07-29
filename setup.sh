#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# setup.sh — One-time setup for new users (llama.cpp build)
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

API_URL="http://localhost:11435"
MAX_WAIT_SECS=500  # 5 minutes — first run downloads ~2.0 GB model

# ── Helpers ──────────────────────────────────────────────────────────────────
log()  { echo "  $*"; }
ok()   { echo "  ✅ $*"; }
warn() { echo "  ⚠️  $*"; }
err()  { echo "  ❌ $*" >&2; }
header() { echo ""; echo "── $* ──────────────────────────────────────────"; }

# ── Banner ───────────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════╗"
echo "║  Document Compiler LLM (llama.cpp) — Setup  ║"
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

if ! docker info &> /dev/null; then
    err "Docker daemon is not running. Please start Docker and try again."
    exit 1
fi
ok "Docker daemon is running"

# ── Check for GPU ─────────────────────────────────────────────────────────────
header "Hardware detection"
if nvidia-smi &> /dev/null && docker info 2>/dev/null | grep -q "nvidia"; then
    GPU=$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -1)
    ok "NVIDIA GPU detected: ${GPU}"
    log "Applying GPU override (docker-compose.gpu.yml)..."
    USE_GPU=true
    COMPOSE_FILES="-f docker-compose.yml -f docker-compose.gpu.yml"
elif [[ "$(uname -m)" == "arm64" ]] && [[ "$(uname)" == "Darwin" ]]; then
    ok "Apple Silicon detected — CPU/Metal acceleration will be used by llama.cpp"
else
    warn "No GPU detected — running in CPU mode"
    log "Inference speed: ~5-15 tokens/second"
fi

# ── Build image ───────────────────────────────────────────────────────────────
header "Setting up Docker image"

if [ "$REBUILD" = true ]; then
    log "Rebuilding image from scratch (--rebuild flag set)..."
    docker compose $COMPOSE_FILES build --no-cache
    ok "Image rebuilt"
else
    log "Building image locally..."
    docker compose $COMPOSE_FILES build
    ok "Image built"
fi

# ── Start service ─────────────────────────────────────────────────────────────
header "Starting LLM service"
log "Starting container doc-compiler-llama..."
docker compose $COMPOSE_FILES up -d
ok "Container started"

# ── Wait for model to be ready ────────────────────────────────────────────────
header "Waiting for model to be ready"
log "This may take several minutes on first run (downloading ~2.0 GB model)..."
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
        echo "    docker compose logs llm     # See container logs"
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

# ── Done ──────────────────────────────────────────────────────────────────────
echo "╔══════════════════════════════════════════════════════════╗"
echo "║         ✅  Setup Complete! (llama.cpp)                 ║"
echo "╠══════════════════════════════════════════════════════════╣"
printf "║  %-56s ║\n" "API URL:   http://localhost:11435/v1"
printf "║  %-56s ║\n" "Model:     doc-compiler (skills injected)"
printf "║  %-56s ║\n" "Base Model: qwen2.5:3b"
printf "║  %-56s ║\n" "API Key:   any string (e.g. 'local')"
echo "╠══════════════════════════════════════════════════════════╣"
echo "║  Quick test:                                            ║"
echo "║    curl http://localhost:11435/v1/chat/completions \\    ║"
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
echo "    docker stats doc-compiler-llama # Monitor memory usage"
echo ""
