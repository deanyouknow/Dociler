FROM ollama/ollama:latest

LABEL maintainer="your-team"
LABEL description="Document-compilation LLM — OpenAI-compatible API"
LABEL version="1.0.0"

# Install curl for healthcheck and startup probing
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy orchestration scripts
COPY entrypoint.sh /entrypoint.sh
COPY healthcheck.sh /healthcheck.sh

# Copy Modelfile template
COPY Modelfile.template /root/Modelfile.template

# Copy default skills (can be overridden via volume mount at runtime)
COPY skills/ /root/skills/

# Make scripts executable
RUN chmod +x /entrypoint.sh /healthcheck.sh

# Liveness probe — Ollama API must be responding
# start-period gives container time for first-run model download
HEALTHCHECK --interval=15s --timeout=5s --start-period=180s --retries=8 \
    CMD /healthcheck.sh

EXPOSE 11434

ENTRYPOINT ["/entrypoint.sh"]
