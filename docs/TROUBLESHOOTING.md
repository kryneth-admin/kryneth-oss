# Troubleshooting Guide

Common issues and how to resolve them.

## Startup Issues

### Port Already in Use

**Error:**
```
thread 'main' panicked at 'Failed to bind to 0.0.0.0:8080: Address already in use'
```

**Solutions:**

Windows:
```powershell
# Find process using port 8080
netstat -ano | findstr :8080

# Kill the process
taskkill /PID <PID> /F
```

macOS/Linux:
```bash
# Find process using port 8080
lsof -i :8080

# Kill the process
kill -9 <PID>

# Or change port
export KRYNETH_PORT=8081
cargo run --release
```

### API Key Not Found

**Error:**
```
thread 'main' panicked at 'Environment variable GROQ_API_KEY not set'
```

**Solution:**
```bash
# Verify environment variable is set
echo $GROQ_API_KEY

# If empty, set it
export GROQ_API_KEY="gsk_..."

# Or use .env file
echo "GROQ_API_KEY=gsk_..." > .env
set -a && source .env && set +a
```

### routing.yaml Not Found

**Error:**
```
thread 'main' panicked at 'routing.yaml not found'
```

**Solution:**
1. Ensure `routing.yaml` exists at project root
2. Check file permissions:
   ```bash
   ls -la routing.yaml
   chmod 644 routing.yaml
   ```
3. Verify YAML syntax is valid:
   ```bash
   # Try parsing with any YAML validator
   cat routing.yaml  # Check for obvious errors
   ```

### Invalid Tenant ID

**Error:**
```
Error: Invalid tenant ID format in routing.yaml
```

**Solution:**
Ensure tenant ID is a valid UUID:
```yaml
# Valid: UUID format
"550e8400-e29b-41d4-a716-446655440000":
  "model-name": ...

# Invalid: Not a UUID
"tenant-1":  # ❌ Wrong format
"000000":    # ❌ Too short
```

## Runtime Issues

### 401 Unauthorized Error

**Error:**
```json
{
  "error": {
    "message": "Unauthorized",
    "type": "auth_error",
    "code": "INVALID_API_KEY"
  }
}
```

**Debugging:**
1. Check API key is correct:
   ```bash
   echo $GROQ_API_KEY
   ```

2. Verify key belongs to correct provider:
   - Groq keys start with `gsk_`
   - Cohere keys start with `h28_` or `h22_`
   - OpenAI keys start with `sk-`

3. Check routing.yaml has correct `api_key_alias`:
   ```yaml
   targets:
     - provider_name: "groq"
       api_key_alias: "GROQ_API_KEY"  # Match this exactly
   ```

4. Test API key directly:
   ```bash
   curl -H "Authorization: Bearer $GROQ_API_KEY" \
     https://api.groq.com/openai/v1/models
   ```

### 503 Service Unavailable

**Error:**
```json
{
  "error": {
    "message": "All upstream providers failed",
    "type": "provider_error",
    "code": "CIRCUIT_BREAKER_OPEN"
  }
}
```

**Causes & Solutions:**

1. **Provider is down:**
   ```bash
   # Check provider status page
   # Groq: https://status.groq.com
   # OpenAI: https://status.openai.com
   # etc.
   ```

2. **Network connectivity issue:**
   ```bash
   # Test connection to provider
   curl https://api.groq.com/openai/v1/models
   
   # Check firewall/proxy
   ping api.groq.com
   ```

3. **Rate limit quota exceeded:**
   - Contact provider support
   - Check billing dashboard
   - Wait for quota reset

4. **All failover targets exhausted:**
   - Verify you have at least one working target in routing.yaml
   - Check logs: `RUST_LOG=debug cargo run`

### Tool Storm Threshold Exceeded

**Error:**
```json
{
  "error": {
    "message": "Tool storm threshold exceeded",
    "type": "tool_limit_error",
    "code": "MAX_TOOL_CALLS_EXCEEDED"
  }
}
```

**Debugging:**

1. Check current settings:
   ```bash
   echo $MAX_SESSION_TOOL_CALLS
   ```

2. Review agent behavior:
   - Is agent making too many tool calls in parallel?
   - Is agent stuck in recursive loop?
   - Are tool calls actually needed?

3. Adjust limits if needed:
   ```bash
   export MAX_SESSION_TOOL_CALLS=50  # Increase threshold
   ```

4. Fix the root cause:
   - Improve agent prompt to reduce tool calls
   - Add early stopping conditions
   - Implement better tool selection logic

### Infinite Loop Detection Triggered

**Error:**
```json
{
  "error": {
    "message": "Infinite loop detected - identical tool calls repeated",
    "type": "safety_error",
    "code": "LOOP_DETECTED"
  }
}
```

**Debugging:**

1. Check current setting:
   ```bash
   echo $MAX_IDENTICAL_TOOL_CALLS
   ```

2. Identify the repeated tool:
   - Enable debug logging: `RUST_LOG=debug`
   - Look for repeating tool name and arguments in logs

3. Example issue:
   ```
   Call 1: get_weather(location="NYC")
   Call 2: get_weather(location="NYC")  ← Same call
   Call 3: get_weather(location="NYC")  ← Same call
   Call 4: get_weather(location="NYC")  ← Same call
   Call 5: get_weather(location="NYC")  ← Same call
   Call 6: get_weather(location="NYC")  ← BLOCKED
   ```

4. Fix the agent:
   ```python
   # Bad: Agent might retry on failure
   for i in range(10):
       result = get_weather(location)  # Keep retrying same location
   
   # Good: Try different approach on failure
   locations = ["NYC", "LA", "Chicago"]
   for loc in locations:
       result = get_weather(location=loc)  # Try different locations
   ```

5. Adjust threshold if needed:
   ```bash
   export MAX_IDENTICAL_TOOL_CALLS=10  # Increase tolerance
   ```

## Performance Issues

### High Latency (P99 > 5ms)

**Debugging:**

1. Check system resources:
   ```bash
   # CPU and memory usage
   top      # Linux/macOS
   # or
   Get-Process | Sort CPU -Descending  # Windows
   ```

2. Enable debug logging:
   ```bash
   RUST_LOG=debug cargo run --release 2>&1 | grep -i latency
   ```

3. Check for hot spots:
   - Large JSON payloads?
   - Many concurrent requests?
   - Cache misses?

4. Optimization steps:
   ```bash
   # Increase worker threads
   export TOKIO_WORKER_THREADS=16
   
   # Increase cache size
   export L1_CACHE_SIZE=50000
   
   # Run benchmark
   cargo run --example stress_test
   ```

### Memory Leak

**Symptoms:**
- Memory usage grows over time
- Process crashes after hours

**Investigation:**

1. Monitor memory:
   ```bash
   # Linux
   watch -n 1 'ps aux | grep kryneth'
   
   # macOS
   while true; do ps aux | grep kryneth; sleep 1; done
   
   # Windows
   Get-Process kryneth | Select-Object @{Name="Memory(MB)";Expression={$_.WorkingSet/1MB}}
   ```

2. Enable detailed logging:
   ```bash
   RUST_LOG=trace cargo run --release 2>&1 | tee kryneth.log
   ```

3. Check for known issues:
   - Moka cache configuration
   - DashMap size limits
   - Connection pooling

4. Report if confirmed:
   - Share logs and reproduction steps
   - GitHub issue with system info

### Slow Cold Start

**Symptoms:**
- First request takes > 100ms

**Causes & Solutions:**

1. **Lazy initialization:**
   - First request triggers setup
   - Compile-time features loading
   - Cache population

2. **Reduce latency:**
   ```bash
   # Pre-warm cache on startup
   cargo run --release -- --preload-cache
   
   # Or send dummy request
   curl http://localhost:8080/health
   ```

## Docker Issues

### Container Won't Start

**Error:**
```
standard_init_linux.go:228: exec user process caused: no such file or directory
```

**Solutions:**

1. Check Dockerfile:
   ```dockerfile
   # Ensure base image exists
   FROM rust:1.70-slim AS builder
   ```

2. Rebuild image:
   ```bash
   docker build --no-cache -t kryneth-gateway:latest .
   ```

3. Check entrypoint:
   ```bash
   docker inspect kryneth-gateway:latest | grep -A 5 Cmd
   ```

### Can't Connect to Gateway

**Error:**
```
Error: Failed to connect to 0.0.0.0:8080
```

**Solutions:**

1. Check port mapping:
   ```bash
   docker ps | grep kryneth
   # Should show: 0.0.0.0:8080->8080/tcp
   ```

2. Try explicit port mapping:
   ```bash
   docker run -p 8080:8080 kryneth-gateway:latest
   ```

3. Check container logs:
   ```bash
   docker logs <container-id>
   ```

### Missing Environment Variables

**Error:**
```
thread 'main' panicked at 'Environment variable not set'
```

**Solution:**

Use `--env-file`:
```bash
docker run --env-file .env kryneth-gateway:latest
```

Or pass explicitly:
```bash
docker run \
  -e GROQ_API_KEY="gsk_..." \
  -e COHERE_API_KEY="h28_..." \
  kryneth-gateway:latest
```

## Debugging Tools

### Enable Verbose Logging
```bash
RUST_LOG=trace cargo run --release
# or
RUST_LOG=debug cargo run --release
```

### Check All Environment Variables
```bash
# Linux/macOS
env | grep -E "KRYNETH|GROQ|COHERE|RATE_LIMIT|MAX_"

# Windows
Get-ChildItem env: | Where-Object {$_.Name -like "*KRYNETH*" -or $_.Name -like "*GROQ*"}
```

### Test Provider Connectivity
```bash
# Groq
curl -H "Authorization: Bearer $GROQ_API_KEY" \
  https://api.groq.com/openai/v1/models

# OpenAI
curl -H "Authorization: Bearer $OPENAI_API_KEY" \
  https://api.openai.com/v1/models

# Cohere
curl -H "Authorization: Bearer $COHERE_API_KEY" \
  https://api.cohere.com/v1/list-models
```

### Validate YAML Configuration
```bash
# Basic syntax check
cat routing.yaml | head -20

# Use online validator: https://www.yamllint.com/
```

## Performance Benchmarking

### Run Built-in Benchmarks
```bash
# Stress test
cargo run --example stress_test --release

# SIMD performance
cargo run --example simd_test --release
```

### Test with Real Requests
```bash
# Concurrency test
ab -n 1000 -c 100 http://localhost:8080/v1/chat/completions

# Using Apache Bench (if installed)
# Or use custom load test
python tests/load_test.py
```

## Getting Help

1. **Check logs:** `RUST_LOG=debug cargo run`
2. **Review docs:** Read [GETTING_STARTED.md](GETTING_STARTED.md)
3. **Check API docs:** See [API.md](API.md)
4. **Report issue:** GitHub Issues with:
   - Error message
   - Configuration (sanitized)
   - System info
   - Steps to reproduce
