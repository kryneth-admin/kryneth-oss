import asyncio
import json
import logging
from typing import Optional
from fastapi import FastAPI, Header, Response, Request
from fastapi.responses import JSONResponse, StreamingResponse
from fastapi.middleware.cors import CORSMiddleware

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("mock_server")

app = FastAPI(title="Kryneth Upstream Mock Server")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

@app.get("/health")
async def health_check():
    return {"status": "ok", "server": "Kryneth Upstream Mock Server"}

async def generate_sse_success():
    """Generates a standard, valid OpenAI SSE completion stream."""
    chunks = [
        {
            "id": "chatcmpl-mock-success",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-mock-success",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"content": "Hello! This is a "}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-mock-success",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"content": "successful stream from the Kryneth E2E mock server."}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-mock-success",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        },
    ]

    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)

    yield "data: [DONE]\n\n"

async def generate_sse_tool_call():
    """Generates an SSE stream containing a tool call invocation."""
    chunks = [
        {
            "id": "chatcmpl-mock-tc",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "get_weather",
                                    "arguments": '{"location": "San Francisco, CA"}',
                                },
                            }
                        ],
                    },
                    "finish_reason": None,
                }
            ],
        },
        {
            "id": "chatcmpl-mock-tc",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        },
    ]

    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)

    yield "data: [DONE]\n\n"

async def generate_sse_github_tool_call():
    """Generates an SSE stream containing a search_repositories tool call invocation."""
    chunks = [
        {
            "id": "chatcmpl-mock-tc-github",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_github_1",
                                "type": "function",
                                "function": {
                                    "name": "search_repositories",
                                    "arguments": '{"query": "kryneth"}',
                                },
                            }
                        ],
                    },
                    "finish_reason": None,
                }
            ],
        },
        {
            "id": "chatcmpl-mock-tc-github",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        },
    ]

    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)

    yield "data: [DONE]\n\n"

async def generate_sse_mid_stream_crash():
    """Emits 2 valid chunks then intentionally aborts connection without [DONE]."""
    chunks = [
        {
            "id": "chatcmpl-mock-crash",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Starting stream... "}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-mock-crash",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"content": "about to crash abruptly!"}, "finish_reason": None}],
        },
    ]

    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)

    logger.warning("Simulating mid-stream crash: aborting SSE generator without [DONE]")
    raise ConnectionResetError("Simulated mid-stream connection abort")

async def generate_sse_deepseek_thinking():
    """Generates an SSE stream with DeepSeek reasoning thinking tags."""
    chunks = [
        {
            "id": "chatcmpl-mock-ds",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "deepseek-r1",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "<think>Analyzing context and verifying rules...</think>\n"}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-mock-ds",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "deepseek-r1",
            "choices": [{"index": 0, "delta": {"content": "Here is the solution to your request."}, "finish_reason": "stop"}],
        },
    ]
    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)
    yield "data: [DONE]\n\n"

async def generate_sse_stock_price_tool_call():
    """Generates an SSE stream containing a web_search tool call invocation."""
    chunks = [
        {
            "id": "chatcmpl-mock-tc-stock",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_stock_1",
                                "type": "function",
                                "function": {
                                    "name": "web_search",
                                    "arguments": '{"query": "AMZN stock price"}',
                                },
                            }
                        ],
                    },
                    "finish_reason": None,
                }
            ],
        },
        {
            "id": "chatcmpl-mock-tc-stock",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        },
    ]

    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)

    yield "data: [DONE]\n\n"

async def generate_sse_sql_tool_call():
    """Generates an SSE stream containing a execute_sql tool call invocation."""
    chunks = [
        {
            "id": "chatcmpl-mock-tc-sql",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_sql_1",
                                "type": "function",
                                "function": {
                                    "name": "execute_sql",
                                    "arguments": '{"query": "INSERT INTO users VALUES (1)"}',
                                },
                            }
                        ],
                    },
                    "finish_reason": None,
                }
            ],
        },
        {
            "id": "chatcmpl-mock-tc-sql",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        },
    ]

    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)

    yield "data: [DONE]\n\n"

async def generate_sse_stock_price_final():
    """Generates final SSE stream text for stock price tool result."""
    chunks = [
        {
            "id": "chatcmpl-mock-final",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "The stock price is $278"}, "finish_reason": None}],
        },
        {
            "id": "chatcmpl-mock-final",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        },
    ]
    for chunk in chunks:
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.05)
    yield "data: [DONE]\n\n"

@app.post("/v1/fail-primary/chat/completions")
async def chat_completions_fail_primary(request: Request):
    """Primary failover target that intentionally returns 429 Rate Limit to trigger circuit breaker hot-swap."""
    logger.warning("Simulating Primary Target Failure: Returning 429 Too Many Requests")
    return JSONResponse(
        status_code=429,
        content={"error": {"message": "Primary quota exhausted", "type": "insufficient_quota"}},
    )

@app.post("/v1/chat/completions")
async def chat_completions(
    request: Request,
    x_test_scenario: Optional[str] = Header(None, alias="X-Test-Scenario"),
):
    scenario = (x_test_scenario or "").strip().lower()
    logger.info(f"Incoming /v1/chat/completions request with scenario: '{scenario}'")

    if scenario in ["rate-limit", "trigger-429"]:
        logger.info("Returning 429 Rate Limit scenario response")
        return JSONResponse(
            status_code=429,
            content={
                "error": {
                    "message": "Upstream rate limit exceeded (Simulated Chaos)",
                    "type": "requests",
                    "param": None,
                    "code": "rate_limit_exceeded",
                }
            },
        )

    if scenario in ["server-error", "trigger-500"]:
        logger.info("Returning 500 Server Error scenario response")
        return JSONResponse(
            status_code=500,
            content={"error": {"message": "Upstream 500 Internal Server Error (Simulated Chaos)"}},
        )

    if scenario == "mid-stream-crash":
        logger.info("Starting mid-stream crash scenario stream")
        return StreamingResponse(
            generate_sse_mid_stream_crash(),
            media_type="text/event-stream",
            headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
        )

    if scenario == "tool-call":
        logger.info("Starting tool-call scenario stream")
        return StreamingResponse(
            generate_sse_tool_call(),
            media_type="text/event-stream",
            headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
        )

    if scenario == "github-tool-call":
        logger.info("Returning static (buffered) github-tool-call response")
        return JSONResponse(
            status_code=200,
            content={
                "id": "chatcmpl-mock-tc-github",
                "object": "chat.completion",
                "created": 1700000000,
                "model": "gpt-4o",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": None,
                            "tool_calls": [
                                {
                                    "id": "call_github_1",
                                    "type": "function",
                                    "function": {
                                        "name": "search_repositories",
                                        "arguments": "{\"query\": \"kryneth\"}"
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }
                ],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 12,
                    "total_tokens": 22
                }
            }
        )
    if scenario == "execute-sql-json":
        logger.info("Returning static (buffered) execute-sql-json response for MCP fan-out testing")
        return JSONResponse(
            status_code=200,
            content={
                "id": "chatcmpl-mock-tc-sql-json",
                "object": "chat.completion",
                "created": 1700000000,
                "model": "gpt-4o",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": None,
                            "tool_calls": [
                                {
                                    "id": "call_sql_json_1",
                                    "type": "function",
                                    "function": {
                                        "name": "execute_sql",
                                        "arguments": "{\"query\": \"INSERT INTO users VALUES (1)\"}"
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }
                ],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 12,
                    "total_tokens": 22
                }
            }
        )


    if scenario == "deepseek-thinking":
        logger.info("Starting DeepSeek reasoning thinking scenario stream")
        return StreamingResponse(
            generate_sse_deepseek_thinking(),
            media_type="text/event-stream",
            headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
        )

    # Stateful Agent Loop Inspection
    try:
        raw_body = await request.body()
        if raw_body:
            body = json.loads(raw_body.decode("utf-8"))
            messages = body.get("messages", [])
            if messages:
                last_msg = messages[-1]
                role = str(last_msg.get("role", "")).lower()
                content_str = str(last_msg.get("content", ""))

                logger.info(f"Mock server inspecting conversation state: last_msg role='{role}' content='{content_str[:60]}...' total_messages={len(messages)}")

                if role == "tool" or any(m.get("role") == "tool" for m in messages):
                    logger.info("Stateful trigger: Tool result received in messages -> Returning final stock price SSE answer")
                    return StreamingResponse(
                        generate_sse_stock_price_final(),
                        media_type="text/event-stream",
                        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
                    )

                if role == "user" and ("stock price" in content_str.lower() or "amzn" in content_str.lower()):
                    logger.info("Stateful trigger: User asked for AMZN stock price -> Returning web_search tool_calls SSE")
                    return StreamingResponse(
                        generate_sse_stock_price_tool_call(),
                        media_type="text/event-stream",
                        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
                    )

                if role == "user" and "run sql" in content_str.lower():
                    logger.info("Stateful trigger: User asked for run sql -> Returning execute_sql tool_calls SSE")
                    return StreamingResponse(
                        generate_sse_sql_tool_call(),
                        media_type="text/event-stream",
                        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
                    )
    except Exception as e:
        logger.warning(f"Non-JSON or empty stream body in mock server: {e}")

    # Default & success-stream scenario
    logger.info("Starting success-stream scenario stream")
    return StreamingResponse(
        generate_sse_success(),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )

@app.post("/v1/chat")
async def cohere_chat(request: Request):
    """Cohere /v1/chat endpoint simulator."""
    logger.info("Incoming Cohere /v1/chat request")
    return JSONResponse(
        status_code=200,
        content={
            "response_id": "cohere-mock-id",
            "text": "Hello from Cohere via Kryneth Gateway!",
            "generation_id": "gen-12345",
            "token_count": {"input_tokens": 10, "output_tokens": 10},
        },
    )

@app.post("/v1/messages")
async def anthropic_messages(
    request: Request,
    x_test_scenario: Optional[str] = Header(None, alias="X-Test-Scenario"),
):
    """Anthropic Claude /v1/messages endpoint simulator."""
    scenario = (x_test_scenario or "").strip().lower()
    logger.info(f"Incoming Anthropic /v1/messages request with scenario: '{scenario}'")

    if scenario == "anthropic-tool-use":
        return JSONResponse(
            status_code=200,
            content={
                "id": "msg_01AnthropicMock",
                "type": "message",
                "role": "assistant",
                "model": "claude-3-5-sonnet-20241022",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_01WebSearch",
                        "name": "web_search",
                        "input": {"query": "Kryneth Gateway AI"},
                    }
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 25, "output_tokens": 15},
            },
        )

    return JSONResponse(
        status_code=200,
        content={
            "id": "msg_01AnthropicSuccess",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-5-sonnet-20241022",
            "content": [{"type": "text", "text": "Hello from Anthropic Claude via Kryneth Gateway!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 12},
        },
    )

@app.post("/v1beta/models/{model_name}:streamGenerateContent")
@app.post("/v1beta/models/{model_name}:generateContent")
async def gemini_generate_content(
    model_name: str,
    request: Request,
    x_test_scenario: Optional[str] = Header(None, alias="X-Test-Scenario"),
):
    """Google Gemini endpoint simulator."""
    scenario = (x_test_scenario or "").strip().lower()
    logger.info(f"Incoming Gemini generateContent request for model '{model_name}' with scenario: '{scenario}'")

    if scenario == "gemini-function-call":
        return JSONResponse(
            status_code=200,
            content={
                "candidates": [
                    {
                        "content": {
                            "parts": [
                                {
                                    "functionCall": {
                                        "name": "execute_sql",
                                        "args": {"query": "SELECT * FROM metrics"},
                                    }
                                }
                            ],
                            "role": "model",
                        },
                        "finishReason": "STOP",
                    }
                ]
            },
        )

    return JSONResponse(
        status_code=200,
        content={
            "candidates": [
                {
                    "content": {
                        "parts": [{"text": "Hello from Google Gemini via Kryneth Gateway!"}],
                        "role": "model",
                    },
                    "finishReason": "STOP",
                }
            ]
        },
    )

@app.post("/mcp/messages")
async def mcp_messages(
    request: Request,
    x_test_scenario: Optional[str] = Header(None, alias="X-Test-Scenario"),
):
    scenario = (x_test_scenario or "").strip().lower()
    logger.info(f"Incoming /mcp/messages request with scenario: '{scenario}'")

    if scenario in ["mcp-timeout", "execute-sql-json"]:
        logger.warning(f"Simulating MCP timeout ({scenario}): sleeping for 10 seconds (exceeds Kryneth 5s firewall)...")
        await asyncio.sleep(10)
        return JSONResponse(
            status_code=200,
            content={
                "jsonrpc": "2.0",
                "result": {"content": [{"type": "text", "text": "Delayed response after 10s"}]},
                "id": 1,
            },
        )

    return JSONResponse(
        status_code=200,
        content={
            "jsonrpc": "2.0",
            "result": {"content": [{"type": "text", "text": "AMZN: $278"}]},
            "id": 1,
        },
    )

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=9090)
