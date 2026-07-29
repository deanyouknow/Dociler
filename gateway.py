import os
import httpx
from fastapi import FastAPI, Request, UploadFile, File, Form, HTTPException
from fastapi.responses import StreamingResponse

app = FastAPI(
    title="Document Compiler LLM Gateway",
    description="Gateway to handle Markdown file uploads and proxy requests to Ollama",
    version="1.0.0"
)

# Ollama internally runs on port 11435 inside the container
OLLAMA_URL = os.environ.get("OLLAMA_BACKEND", "http://127.0.0.1:11435")

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
        
    # 3. Construct chat message with context document injected
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
    
    # 4. Send to Ollama
    if stream:
        async def event_generator():
            async with httpx.AsyncClient(timeout=300.0) as client:
                async with client.stream(
                    "POST",
                    f"{OLLAMA_URL}/v1/chat/completions",
                    json=payload,
                    headers=headers,
                    timeout=300.0
                ) as r:
                    if r.status_code >= 400:
                        yield f"Error: {await r.aread()}".encode("utf-8")
                        return
                    async for chunk in r.aiter_bytes():
                        yield chunk
        return StreamingResponse(event_generator(), media_type="text/event-stream")
    else:
        async with httpx.AsyncClient(timeout=300.0) as client:
            try:
                r = await client.post(
                    f"{OLLAMA_URL}/v1/chat/completions",
                    json=payload,
                    headers=headers,
                    timeout=300.0
                )
                r.raise_for_status()
                return r.json()
            except httpx.HTTPStatusError as e:
                raise HTTPException(status_code=e.response.status_code, detail=e.response.text)
            except Exception as e:
                raise HTTPException(status_code=500, detail=f"Error connecting to Ollama: {str(e)}")

# Catch-all proxy route to forward all other requests (completions, models listing, etc.)
@app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE"])
async def wildcard_proxy(path: str, request: Request):
    url = f"{OLLAMA_URL}/{path}"
    headers = dict(request.headers)
    
    # Remove host header so httpx calculates it properly for the destination
    headers.pop("host", None)
    
    body = await request.body()
    params = request.query_params
    
    async with httpx.AsyncClient(timeout=300.0) as client:
        try:
            req = client.build_request(
                method=request.method,
                url=url,
                headers=headers,
                params=params,
                content=body
            )
            resp = await client.send(req, stream=True)
            
            # Forward relevant response headers, omitting hop-by-hop & encoding headers
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
            raise HTTPException(status_code=500, detail=f"Proxy error connecting to Ollama: {str(e)}")
