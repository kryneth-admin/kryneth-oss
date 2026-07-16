---
icon: "cpu"
---
# Model Context Protocol (MCP) Router

To securely connect autonomous LLM agents with local databases, tools, or internal APIs, Kryneth incorporates an MCP router ("Tunnel 3"). It translates LLM tool call schemas into Model Context Protocol invocations, routing them over Server-Sent Events (SSE).

---

## 1. Zero-Copy Routing & Tool Execution

When an upstream LLM decides to trigger a tool, it issues a standard tool call payload. Kryneth intercepts this payload, translates the request structure, and executes the tool over the SSE tunnel.

```mermaid
sequenceDiagram
    participant LLM as Upstream LLM
    participant Kryneth as Kryneth Gateway
    participant MCP as MCP SSE Server
    
    LLM->>Kryneth: Request Tool Call: query_db(id=10)
    Note over Kryneth: Lookup tool in registry
    Kryneth->>MCP: Post HTTP SSE /call_tool { "name": "query_db", "arguments": {"id":10} }
    MCP-->>Kryneth: Return tool response
    Kryneth-->>LLM: Forward tool results
```

---

## 2. Practical MCP Registry Configuration

Connected MCP servers are registered dynamically using the **`MCP_TOOL_REGISTRY`** environment variable or via the configuration control plane. 

### `MCP_TOOL_REGISTRY` Environment Variable Format
This variable must contain a valid JSON object mapping tool names directly to their SSE endpoint URLs:

```json
{
  "fetch_customer_data": "http://localhost:9000/sse",
  "execute_sql_query": "http://localhost:9000/sse",
  "send_slack_alert": "http://slack-mcp.internal.net/sse"
}
```

*   **Mapping Rule**: Multiple tools can map to the same SSE endpoint (e.g. `fetch_customer_data` and `execute_sql_query` both hit the same server instance at `:9000/sse`).

---

## 3. Python-Based MCP SSE Server Example

Below is a complete, lightweight Python-based MCP server using the official `mcp` SDK, exposing an SSE connection endpoint that Kryneth can query.

### Installation
```bash
pip install mcp[cli] fastapi uvicorn
```

### Server Implementation (`server.py`)
```python
import uvicorn
from fastapi import FastAPI
from mcp.server.fastapi import QueueServer
from mcp.types import Tool, TextContent

app = FastAPI(title="Kryneth Custom MCP Server")
# Create the MCP QueueServer runner
mcp_server = QueueServer(name="custom-db-tools")

# Register tools exposed to the gateway
@mcp_server.list_tools()
async def handle_list_tools() -> list[Tool]:
    return [
        Tool(
            name="fetch_customer_data",
            description="Fetches basic client profile data by customer ID.",
            inputSchema={
                "type": "object",
                "properties": {
                    "customer_id": {"type": "string", "description": "UUID string of the target customer"}
                },
                "required": ["customer_id"],
            }
        )
    ]

# Handle execution requests forwarded by Kryneth
@mcp_server.call_tool()
async def handle_call_tool(name: str, arguments: dict) -> list[TextContent]:
    if name == "fetch_customer_data":
        cust_id = arguments.get("customer_id")
        # In a real app, query database here
        mock_profile = f"Customer Profile for {cust_id}: Status=Active, Tier=Enterprise"
        return [TextContent(type="text", text=mock_profile)]
    raise ValueError(f"Tool {name} not found")

# Bind the MCP SSE routes to FastAPI
mcp_server.link_to_fastapi(app, sse_path="/sse", messages_path="/messages")

if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=9000)
```

---

## 4. Bounded Concurrency & Hard Timeouts

To prevent the gateway from locking up during high-velocity parallel tool calls, Kryneth applies strict execution boundaries:
*   **Bounded Parallel Fan-out**: Outbound tool executions are capped at a maximum of **10 parallel requests** (`futures::stream::buffer_unordered(10)`) per agent session. This avoids overloading local network interfaces or downstream services.
*   **Per-Call Timeout**: Every outbound tool invocation has a hard **5-second timeout**. If a tool fails to resolve in 5 seconds, it is terminated and returns a structured error:
    ```json
    { "error": "MCP_TIMEOUT" }
    ```
    This ensures that slower or hung tool calls do not block other concurrent tool executions.

---

## 5. Merge Schema Format

Upon completing tool runs, Kryneth gathers the responses into a standardized JSON array format before passing them back to the upstream LLM:
```json
[
  {
    "tool": "fetch_customer_data",
    "result": { "text": "Customer Profile for 123: Status=Active, Tier=Enterprise" },
    "latency_ms": 142,
    "success": true
  }
]
```
If a tool returns non-JSON raw text, Kryneth sanitizes and binds it under a standard `"text"` JSON key to maintain schema validity.

---

## 6. Two-Phase Semantic Lazy Schema Loading

Autonomous agent setups often load massive schemas containing descriptions and parameter formats for hundreds of tools. This "MCP Tax" exhausts LLM context windows and spikes token consumption.

To solve this, Kryneth implements a **Two-Phase Semantic Lazy Schema Loading** system:
1.  **Phase A (Lazy Injection)**: The gateway intercepts outgoing `tools` definitions. It strips out the massive `parameters` block, replacing it with a single semantic description line. This reduces context window token load significantly (e.g. from 300 to 15 tokens per tool definition).
2.  **Phase B (Schema on Demand)**: If the LLM requests detailed parameters for a tool, Kryneth intercepts the query, injects the parameters block back in from the local schema cache, and feeds it to the LLM.

---

## 7. Configuration Reference

Configure these environment variables in your deployment setup:

| Env Variable | Requirement | Default | Description |
| :--- | :--- | :--- | :--- |
| `MCP_TOOL_REGISTRY` | **Required** | *Empty* | JSON object mapping tool names directly to their SSE endpoint URLs. |
| `MCP_TOOL_SCHEMA_REGISTRY` | **Optional** | *Empty* | JSON array containing tool descriptors and schema structures for lazy loading. |
