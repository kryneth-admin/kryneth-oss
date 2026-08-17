# Kryneth Gateway - End-to-End (E2E) Integration Sandbox

This sandbox provides a complete local E2E testing environment to detect and prevent frontend SDK integration mismatches (silent bugs, blank screens, stream drops) under real-world upstream chaos.

## Architecture

```text
┌─────────────────────────┐         ┌─────────────────────────┐         ┌─────────────────────────┐
│  Automated Client       │         │     Kryneth Gateway     │         │   Upstream Mock Server  │
│  Simulator (Node.js)    │ ──────> │       (Rust / Axum)     │ ──────> │    (Python / FastAPI)   │
│  Vercel AI SDK / OpenAI │         │       Port 8080         │         │       Port 9090         │
└─────────────────────────┘         └─────────────────────────┘         └─────────────────────────┘
```

---

## Step 1: Start the Upstream Mock Server (Python / Virtualenv)

We provide automated scripts to create a dedicated Python virtual environment (`.venv`), install dependencies, and launch the server on port **9090**:

### Windows (PowerShell)
```powershell
.\mock_server\setup_venv.ps1
```

### Linux / macOS (Bash)
```bash
chmod +x ./mock_server/setup_venv.sh
./mock_server/setup_venv.sh
```

### Manual venv Setup (Alternative)
```bash
# 1. Create virtual environment
py -m venv mock_server/.venv

# 2. Activate virtual environment
# Windows:
.\mock_server\.venv\Scripts\activate
# Linux/macOS:
source mock_server/.venv/bin/activate

# 3. Install requirements and start server
pip install -r mock_server/requirements.txt
uvicorn mock_server.main:app --host 0.0.0.0 --port 9090 --reload
```

*Health Check*: Verify `http://localhost:9090/health` returns `{"status": "ok"}`.

---

## Step 2: Start Kryneth AI Gateway (Rust)

1. Ensure environment variables or `routing.yaml` point upstream requests to the Python Mock Server (`http://localhost:9090`).
2. Launch Kryneth Gateway on port **8080**:
   ```bash
   cargo run --bin kryneth_gateway
   ```

---

## Step 3: Run the Automated Client Simulator (Node.js)

1. Navigate to the `e2e_clients/` directory and install npm dependencies:
   ```bash
   cd e2e_clients
   npm install
   ```
2. Run the automated integration test runner:
   ```bash
   npm test
   ```

---

## Supported Test Scenarios (`X-Test-Scenario` Header)

| Scenario Header | Description / Upstream Behavior | Expected Gateway / Client Behavior |
| :--- | :--- | :--- |
| `success-stream` | Emits valid OpenAI SSE stream chunks & `[DONE]`. | Vercel AI SDK reconstructs full text stream without drops. |
| `tool-call` | Emits SSE stream with `tool_calls` delta (`get_weather`). | OpenAI SDK parses function name & arguments. |
| `mid-stream-crash` | Emits 2 SSE chunks then forcibly aborts connection without `[DONE]`. | Gateway handles disconnect gracefully; client SDK catches error. |
| `rate-limit` | Returns HTTP 429 Rate Limit JSON response. | Gateway forwards 429 status code cleanly to client. |
| `mcp-timeout` | Delays `/mcp/messages` response for 10s. | Gateway 5s idempotency/timeout firewall blocks delayed response. |
