# Kryneth Gateway Load Testing & Performance Benchmarks

This directory contains `k6` scripts and setup instructions to load test and benchmark the Kryneth Gateway. All tests are designed to run locally using a high-throughput mock upstream server, requiring **zero** external API costs or internet connection.

## 🏗️ Architecture

```
                                  +-----------------------------+
                                  |       Kryneth Ingress       |
                                  |    (Axum Gateway, :8080)    |
                                  +--------------+--------------+
                                                 |
                        +------------------------+------------------------+
                        | (PII Scan / Loop Trap) | (Cache Hit)            |
                        v                                                 v
          +-------------+-------------+                     +-------------+-------------+
          |    Compliance Redirection |                     |  L1 Exact / Semantic Cache |
          |    (:8090/compliance)     |                     |  (In-Memory Moka / Dash)  |
          +-------------+-------------+                     +-------------+-------------+
                        |                                                 |
                        v                                                 | (Fast path)
          +-------------+-------------+                                   |
          |     Mock Upstream LLM     |                                   |
          |       (:8090/v1)          |                                   |
          +-------------+-------------+                                   |
                        |                                                 |
                        +------------------------+------------------------+
                                                 v
                                  +--------------+--------------+
                                  |           k6 Client         |
                                  +-----------------------------+
```

---

## 🚀 Step-by-Step Execution Guide

### Step 1: Start the Mock Upstream Server
The mock upstream server simulates both the LLM provider (`/v1/chat/completions`) and the compliance scanner (`/api/v1/compliance/redact`) with sub-millisecond response latencies.

Run it in a separate terminal:
```bash
cargo run --example mock_upstream
```

---

### Step 2: Start the Kryneth Gateway in Benchmark Mode
Launch the gateway with the benchmark routing configuration (`routing.bench.yaml`) and point the compliance scanner to the mock server.

Run it in a separate terminal:

**On PowerShell (Windows):**
```powershell
$env:ROUTING_CONFIG_PATH="routing.bench.yaml"
$env:COMPLIANCE_URL="http://localhost:8090"
$env:KRYNETH_VALID_KEYS="re_live_dev_123"
cargo run
```

**On bash (Linux/macOS):**
```bash
ROUTING_CONFIG_PATH="routing.bench.yaml" COMPLIANCE_URL="http://localhost:8090" KRYNETH_VALID_KEYS="re_live_dev_123" cargo run
```

---

### Step 3: Run the k6 Benchmarks

Execute any of the following scripts using `k6`.

#### 1. Cache Hit Benchmark (Fast Path)
Measures the absolute limits of the Axum router and Kryneth's in-memory L1 cache. Sends 100% identical requests.
```bash
k6 run tests/load/cache_hit_benchmark.js
```
* **Expectation:** P95 latency `< 2.5ms` (often `< 1ms` local). RPS in the thousands depending on CPU cores.

#### 2. PII Scrubbing Benchmark
Varies the prompt content on every request to bypass caching, forcing Kryneth to run its regex patterns and call the compliance redaction endpoint for every request.
```bash
k6 run tests/load/pii_scrubbing_benchmark.js
```
* **Expectation:** Tests the throughput of regex matching and connection pooling to the compliance server.

#### 3. Agent Loop Detection Benchmark
Simulates recursive loop conditions by sending the same tool-call signature under a static session ID.
```bash
k6 run tests/load/loop_detection_benchmark.js
```
* **Expectation:** The first 5 requests per VU session succeed (HTTP 200). All subsequent requests are intercepted by Kryneth's Loop Trap and rejected with `HTTP 429 Too Many Requests` (`AGENT_RUNAWAY_LOOP`).

#### 4. Mixed Workload Benchmark
Simulates a real production load: 60% cache hits, 30% cache misses (routing to mock LLM), and 10% looping/abusive agents.
```bash
k6 run tests/load/mixed_workload_benchmark.js
```

---

## 📈 Analyzing Metrics

When a test completes, look at these key `k6` indicators:
- **`http_req_duration`**: Time taken to receive the response (look at `avg`, `p(90)`, and `p(95)`).
- **`http_reqs`**: The count and rate (req/s) of total requests handled by the gateway.
- **`checks`**: The percentage of requests that passed assertions (e.g. status code, cache-status headers).
