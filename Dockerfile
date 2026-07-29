FROM ghcr.io/ggerganov/llama.cpp:server AS llamacpp-bin

FROM ubuntu:22.04

LABEL maintainer="your-team"
LABEL description="Document-compilation LLM — llama.cpp-based OpenAI-compatible API"
LABEL version="2.0.0"

ENV DEBIAN_FRONTEND=noninteractive

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    wget \
    ca-certificates \
    python3 \
    python3-pip \
    libgomp1 \
    libvulkan1 \
    libcurl4 \
    && rm -rf /var/lib/apt/lists/*

# Install python dependencies for gateway
RUN pip3 install --no-cache-dir --break-system-packages fastapi uvicorn python-multipart httpx

# Copy llama-server binary from official llama.cpp image
COPY --from=llamacpp-bin / /tmp/llamacpp-files/
RUN cp /tmp/llamacpp-files/llama-server /usr/local/bin/llama-server 2>/dev/null || \
    cp /tmp/llamacpp-files/bin/llama-server /usr/local/bin/llama-server 2>/dev/null || \
    find /tmp/llamacpp-files/ -name llama-server -exec cp {} /usr/local/bin/llama-server \; && \
    rm -rf /tmp/llamacpp-files

# Copy orchestration scripts and gateway
COPY entrypoint.sh /entrypoint.sh
COPY healthcheck.sh /healthcheck.sh
COPY gateway.py /root/gateway.py
COPY skills/ /root/skills/

# Make binaries and scripts executable
RUN chmod +x /entrypoint.sh /healthcheck.sh /usr/local/bin/llama-server 2>/dev/null || true

# Liveness probe
HEALTHCHECK --interval=15s --timeout=5s --start-period=180s --retries=8 \
    CMD /healthcheck.sh

EXPOSE 11434

ENTRYPOINT ["/entrypoint.sh"]
