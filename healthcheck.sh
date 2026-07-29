#!/bin/bash
# healthcheck.sh — Docker HEALTHCHECK probe for llama.cpp + Gateway API
# Returns 0 (healthy) if the API is responding, 1 (unhealthy) otherwise

API_URL="http://localhost:11434"

# Check Gateway /v1/models endpoint
if ! curl -sf "${API_URL}/v1/models" > /dev/null 2>&1; then
    exit 1
fi

exit 0
