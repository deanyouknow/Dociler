#!/bin/bash
# healthcheck.sh — Docker HEALTHCHECK probe for the Ollama API
# Returns 0 (healthy) if the API is responding, 1 (unhealthy) otherwise

API_URL="http://localhost:11434"

# Check Ollama API is up
if ! curl -sf "${API_URL}/api/tags" > /dev/null 2>&1; then
    exit 1
fi

# Check that our custom model exists
if ! curl -sf "${API_URL}/api/tags" | grep -q "doc-compiler"; then
    # Model may still be loading — return healthy to avoid premature restarts
    # The start-period in HEALTHCHECK gives us time before this matters
    exit 0
fi

exit 0
