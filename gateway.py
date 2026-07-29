import os
import glob
import time
import httpx
from fastapi import FastAPI, Request, UploadFile, File, Form, HTTPException
from fastapi.responses import StreamingResponse, JSONResponse

app = FastAPI(
    title="Document Compiler LLM Gateway (llama.cpp)",
    description="Gateway to handle Markdown file uploads, skills system prompt injection, and proxy requests to llama-server",
    version="2.0.0"
)

# llama-server runs internally on port 8080 inside the container
LLAMA_URL = os.environ.get("LLAMA_BACKEND", "http://127.0.0.1:8080")
SKILLS_DIR = os.environ.get("SKILLS_DIR", "/root/skills")

def load_skills_prompt() -> str:
    """Auto-discover and concatenate skills.md and any skills-*.md or *skills*.md files."""
    skills_files = []
    
    # 1. Primary skill file
    primary_skill = os.path.join(SKILLS_DIR, "skills.md")
    if os.path.isfile(primary_skill):
        skills_files.append(primary_skill)

    # 2. Additional skill files (*skills*.md or skills-*.md), excluding primary
    additional_patterns = [
        os.path.join(SKILLS_DIR, "*skills*.md"),
        os.path.join(SKILLS_DIR, "skills-*.md")
    ]
    discovered = set()
    for pattern in additional_patterns:
        for filepath in glob.glob(pattern):
            if os.path.isfile(filepath) and os.path.abspath(filepath) != os.path.abspath(primary_skill):
                discovered.add(filepath)
                
    skills_files.extend(sorted(list(discovered)))

    if not skills_files:
        return "You are a helpful document assistant. Help users read, understand, summarize, and compile documents."

    contents = []
    for sf in skills_files:
        try:
            with open(sf, "r", encoding="utf-8") as f:
                text = f.read().strip()
                if text:
                    contents.append(text)
        except Exception as e:
            print(f"[Gateway] Warning: Failed to read skill file {sf}: {e}")

    return "\n\n---\n\n".join(contents)

# Cache skills system prompt on startup
SKILLS_SYSTEM_PROMPT = load_skills_prompt()

@app.on_event("startup")
def startup_event():
    global SKILLS_SYSTEM_PROMPT
    SKILLS_SYSTEM_PROMPT = load_skills_prompt()
    print(f"[Gateway] Skills system prompt loaded ({len(SKILLS_SYSTEM_PROMPT)} characters)")

@app.get("/v1/models")
async def list_models():
    """Return OpenAI compatible model list with doc-compiler and qwen2.5:3b."""
    now = int(time.time())
    return {
        "object": "list",
        "data": [
            {
                "id": "doc-compiler",
                "object": "model",
                "created": now,
                "owned_by": "local"
            },
            {
                "id": "qwen2.5:3b",
                "object": "model",
                "created": now,
                "owned_by": "local"
            }
        ]
    }

@app.post("/v1/chat/completions")
async def chat_completions(request: Request):
    """Intercept chat completions to inject skills prompt for 'doc-compiler' model."""
    try:
        body = await request.json()
    except Exception:
        raise HTTPException(status_code=400, detail="Invalid JSON body")

    model_requested = body.get("model", "doc-compiler")
    messages = body.get("messages", [])

    # If model is doc-compiler (or default), inject skills prompt
    if model_requested in ["doc-compiler", "doc-compiler-llama", "default"] or not model_requested:
        skills_msg = {"role": "system", "content": SKILLS_SYSTEM_PROMPT}
        
        # Check if first message is already system role
        if messages and messages[0].get("role") == "system":
            # Prepend skills instructions to existing system message
            messages[0]["content"] = f"{SKILLS_SYSTEM_PROMPT}\n\n{messages[0].get('content', '')}"
        else:
            # Insert skills prompt at the start
            messages.insert(0, skills_msg)

    body["messages"] = messages
    stream = body.get("stream", False)
    headers = {"Content-Type": "application/json"}

    if stream:
        async def event_generator():
            async with httpx.AsyncClient(timeout=600.0) as client:
                async with client.stream(
                    "POST",
                    f"{LLAMA_URL}/v1/chat/completions",
                    json=body,
                    headers=headers,
                    timeout=600.0
                ) as r:
                    if r.status_code >= 400:
                        err_body = await r.aread()
                        yield f"Error: {err_body.decode('utf-8')}".encode("utf-8")
                        return
                    async for chunk in r.aiter_bytes():
                        yield chunk
        return StreamingResponse(event_generator(), media_type="text/event-stream")
    else:
        async with httpx.AsyncClient(timeout=600.0) as client:
            try:
                r = await client.post(
                    f"{LLAMA_URL}/v1/chat/completions",
                    json=body,
                    headers=headers,
                    timeout=600.0
                )
                r.raise_for_status()
                res_data = r.json()
                # Rewrite model name in response to match requested alias
                if isinstance(res_data, dict):
                    res_data["model"] = model_requested
                return res_data
            except httpx.HTTPStatusError as e:
                raise HTTPException(status_code=e.response.status_code, detail=e.response.text)
            except Exception as e:
                raise HTTPException(status_code=500, detail=f"Error connecting to llama-server: {str(e)}")

@app.post("/v1/chat/with-file")
async def chat_with_file(
    file: UploadFile = File(..., description="The Markdown (.md, .markdown) or plain text (.txt) file to upload"),
    message: str = Form(..., description="The prompt or question to ask the model about the document"),
    model: str = Form("doc-compiler", description="The custom model to use"),
    stream: bool = Form(False, description="Whether to stream the response"),
    temperature: float = Form(0.2, description="Sampling temperature")
):
    # 1. Validate file extension
    filename = file.filename or ""
    if not filename.lower().endswith((".md", ".markdown", ".txt")):
        raise HTTPException(
            status_code=400,
            detail="Only Markdown (.md, .markdown) or text (.txt) files are supported."
        )
    
    # 2. Read file content
    try:
        content_bytes = await file.read()
        file_content = content_bytes.decode("utf-8")
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"Failed to read file: {str(e)}")
        
    # 3. Construct user prompt with document injected
    prompt_content = f"Context Document ({filename}):\n\n{file_content}\n\nUser Prompt:\n{message}"
    
    payload = {
        "model": model,
        "messages": [
            {"role": "user", "content": prompt_content}
        ],
        "stream": stream,
        "temperature": temperature
    }
    
    headers = {"Content-Type": "application/json"}
    
    # 4. Process via chat_completions logic (injects skills)
    if stream:
        async def event_generator():
            async with httpx.AsyncClient(timeout=600.0) as client:
                # Prepare payload with skills injection
                skills_msg = {"role": "system", "content": SKILLS_SYSTEM_PROMPT}
                payload["messages"].insert(0, skills_msg)
                
                async with client.stream(
                    "POST",
                    f"{LLAMA_URL}/v1/chat/completions",
                    json=payload,
                    headers=headers,
                    timeout=600.0
                ) as r:
                    if r.status_code >= 400:
                        err_body = await r.aread()
                        yield f"Error: {err_body.decode('utf-8')}".encode("utf-8")
                        return
                    async for chunk in r.aiter_bytes():
                        yield chunk
        return StreamingResponse(event_generator(), media_type="text/event-stream")
    else:
        skills_msg = {"role": "system", "content": SKILLS_SYSTEM_PROMPT}
        payload["messages"].insert(0, skills_msg)
        
        async with httpx.AsyncClient(timeout=600.0) as client:
            try:
                r = await client.post(
                    f"{LLAMA_URL}/v1/chat/completions",
                    json=payload,
                    headers=headers,
                    timeout=600.0
                )
                r.raise_for_status()
                res_data = r.json()
                if isinstance(res_data, dict):
                    res_data["model"] = model
                return res_data
            except httpx.HTTPStatusError as e:
                raise HTTPException(status_code=e.response.status_code, detail=e.response.text)
            except Exception as e:
                raise HTTPException(status_code=500, detail=f"Error connecting to llama-server: {str(e)}")

# Catch-all proxy route to forward all other requests (completions, health, etc.)
@app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE"])
async def wildcard_proxy(path: str, request: Request):
    url = f"{LLAMA_URL}/{path}"
    headers = dict(request.headers)
    headers.pop("host", None)
    
    body = await request.body()
    params = request.query_params
    
    async with httpx.AsyncClient(timeout=600.0) as client:
        try:
            req = client.build_request(
                method=request.method,
                url=url,
                headers=headers,
                params=params,
                content=body
            )
            resp = await client.send(req, stream=True)
            
            exclude_headers = {"content-encoding", "content-length", "transfer-encoding", "connection", "keep-alive"}
            resp_headers = {k: v for k, v in resp.headers.items() if k.lower() not in exclude_headers}
            
            async def resp_generator():
                try:
                    async for chunk in resp.aiter_bytes():
                        yield chunk
                finally:
                    await resp.aclose()
            
            return StreamingResponse(
                resp_generator(),
                status_code=resp.status_code,
                headers=resp_headers
            )
        except Exception as e:
            raise HTTPException(status_code=500, detail=f"Proxy error connecting to llama-server: {str(e)}")
