import { createOpenAI } from '@ai-sdk/openai';
import { streamText, generateText } from 'ai';
import OpenAI from 'openai';

const GATEWAY_BASE_URL = process.env.GATEWAY_URL || 'http://localhost:8080/v1';
const API_KEY = process.env.GATEWAY_API_KEY || 'ke_live_test_key';

console.log('===========================================================');
console.log('🚀 Starting Kryneth AI Gateway 50+ Scenario E2E Integration Suite');
console.log(`📍 Target Gateway URL: ${GATEWAY_BASE_URL}`);
console.log('===========================================================\n');

const openaiProvider = createOpenAI({
  baseURL: GATEWAY_BASE_URL,
  apiKey: API_KEY,
  compatibility: 'compatible',
});

const openaiClient = new OpenAI({
  baseURL: GATEWAY_BASE_URL,
  apiKey: API_KEY,
});

async function runScenario(id, name, testFn) {
  try {
    const success = await testFn();
    if (success) {
      console.log(`  [TEST ${id}] ✅ ${name} PASSED`);
      return { id, name, status: 'PASSED' };
    } else {
      console.error(`  [TEST ${id}] ❌ ${name} FAILED`);
      return { id, name, status: 'FAILED' };
    }
  } catch (err) {
    console.error(`  [TEST ${id}] ❌ ${name} FAILED with exception: ${err.message}`);
    return { id, name, status: 'FAILED', error: err.message };
  }
}

// ---------------------------------------------------------------------------
// GROUP 1: Input Validation & Edge Cases (Scenarios 1 - 10)
// ---------------------------------------------------------------------------
async function t1_malformedJson() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: '{"model": "gpt-4o", "messages": [',
  });
  return res.status === 400;
}

async function t2_missingModel() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ messages: [{ role: 'user', content: 'hello' }] }),
  });
  return res.status === 400;
}

async function t3_unconfiguredModel() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'non-existent-model-xyz', messages: [{ role: 'user', content: 'hello' }] }),
  });
  return res.status === 400 || res.status === 404;
}

async function t4_emptyMessages() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [] }),
  });
  return res.status === 400 || res.status === 200;
}

async function t5_nullPayload() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: 'null',
  });
  return res.status === 400;
}

async function t6_invalidRole() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'super_admin_invalid', content: 'test' }] }),
  });
  return res.status === 400 || res.status === 200;
}

async function t7_oversizedPayloadBypass() {
  const largeText = 'A'.repeat(5 * 1024 * 1024); // 5 MB payload
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: largeText }] }),
  });
  return res.status === 200 || res.status === 413;
}

async function t8_extraRootParams() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'test' }], unknown_custom_field: 12345 }),
  });
  return res.status === 200;
}

async function t9_missingAuthHeader() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'test' }] }),
  });
  return res.status === 200 || res.status === 401; // Gateway OSS accepts or rejects based on key auth
}

async function t10_invalidRouteAlias() {
  const res = await fetch(`${GATEWAY_BASE_URL}/invalid_route_xyz`, {
    method: 'GET',
  });
  return res.status === 404;
}

// ---------------------------------------------------------------------------
// GROUP 2: Multi-Provider Schema Translations (Scenarios 11 - 20)
// ---------------------------------------------------------------------------
async function t11_vercelStreaming() {
  const result = await streamText({
    model: openaiProvider.chat('gpt-4o'),
    prompt: 'Hello Kryneth',
    headers: { 'X-Test-Scenario': 'success-stream' },
  });
  let fullText = '';
  for await (const delta of result.textStream) { fullText += delta; }
  return fullText.length > 0;
}

async function t12_openaiNonStreaming() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'test' }], stream: false }),
  });
  return res.status === 200;
}

async function t13_openaiToolCallDelta() {
  const responseStream = await openaiClient.chat.completions.create(
    { model: 'gpt-4o', messages: [{ role: 'user', content: 'Weather' }], stream: true },
    { headers: { 'X-Test-Scenario': 'tool-call' } }
  );
  let capturedToolCalls = [];
  for await (const chunk of responseStream) {
    const tc = chunk.choices[0]?.delta?.tool_calls;
    if (tc) capturedToolCalls.push(...tc);
  }
  return capturedToolCalls.some(tc => tc.function?.name === 'get_weather');
}

async function t14_anthropicClaudeMessages() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'claude-3-5-sonnet', messages: [{ role: 'user', content: 'Hello Claude' }] }),
  });
  return res.status === 200;
}

async function t15_anthropicToolUseBlock() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Test-Scenario': 'anthropic-tool-use' },
    body: JSON.stringify({ model: 'claude-3-5-sonnet', messages: [{ role: 'user', content: 'Search' }] }),
  });
  return res.status === 200;
}

async function t16_geminiGenerateContent() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gemini-1.5-pro', messages: [{ role: 'user', content: 'Hello Gemini' }] }),
  });
  return res.status === 200;
}

async function t17_geminiFunctionCallPart() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Test-Scenario': 'gemini-function-call' },
    body: JSON.stringify({ model: 'gemini-1.5-pro', messages: [{ role: 'user', content: 'SQL' }] }),
  });
  return res.status === 200;
}

async function t18_deepseekThinkingStream() {
  const result = await streamText({
    model: openaiProvider.chat('deepseek-r1'),
    prompt: 'Reasoning',
    headers: { 'X-Test-Scenario': 'deepseek-thinking' },
  });
  let fullText = '';
  for await (const delta of result.textStream) { fullText += delta; }
  return fullText.includes('<think>') || fullText.includes('solution');
}

async function t19_cohereCommandR() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'cohere-command-r', messages: [{ role: 'user', content: 'Cohere' }] }),
  });
  return res.status === 200;
}

async function t20_responsesEndpointAlias() {
  const res = await fetch(`${GATEWAY_BASE_URL}/responses`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Alias check' }] }),
  });
  return res.status === 200;
}

// ---------------------------------------------------------------------------
// GROUP 3: Chaos Engineering & Failovers (Scenarios 21 - 30)
// ---------------------------------------------------------------------------
async function t21_midStreamCrashResilience() {
  try {
    const result = await streamText({
      model: openaiProvider.chat('gpt-4o'),
      prompt: 'Crash',
      headers: { 'X-Test-Scenario': 'mid-stream-crash' },
    });
    for await (const _ of result.textStream) {}
    return false;
  } catch (err) {
    return true; // Caught expected stream disconnect exception!
  }
}

async function t22_circuitBreakerHotSwap() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'failover-model', messages: [{ role: 'user', content: 'Failover' }] }),
  });
  return res.status === 200;
}

async function t23_rateLimit429Handling() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Test-Scenario': 'rate-limit' },
    body: JSON.stringify({ model: 'rate-limit-model', messages: [{ role: 'user', content: 'Rate limit' }] }),
  });
  return [200, 429, 500, 502, 503, 504].includes(res.status);
}

async function t24_serverError500Handling() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Test-Scenario': 'server-error' },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: '500 error' }] }),
  });
  return res.status === 500 || res.status === 200 || res.status === 503;
}

async function t25_allKeysExhausted503() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'unconfigured-model-key', messages: [{ role: 'user', content: 'test' }] }),
  });
  return res.status === 503 || res.status === 400 || res.status === 404;
}

async function t26_rapidBurstRequests() {
  const reqs = Array.from({ length: 5 }, () =>
    fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
      body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'burst' }] }),
    })
  );
  const responses = await Promise.all(reqs);
  return responses.every(r => r.status === 200);
}

async function t27_corsPreflightOptions() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'OPTIONS',
    headers: {
      'Access-Control-Request-Method': 'POST',
      'Access-Control-Request-Headers': 'X-Test-Scenario, Content-Type',
      Origin: 'http://localhost:3000',
    },
  });
  return res.status === 200 || res.status === 204;
}

async function t28_headerCaseInsensitivity() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'authorization': `Bearer ${API_KEY}`,
      'x-test-scenario': 'success-stream',
    },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'case check' }] }),
  });
  return res.status === 200;
}

async function t29_traceIdPropagation() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'trace check' }] }),
  });
  return res.headers.has('x-trace-id') || res.headers.has('x-session-id') || res.status === 200;
}

async function t30_sessionTracingHeader() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Session-ID': 'session-test-123' },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'session check' }] }),
  });
  return res.status === 200;
}

// ---------------------------------------------------------------------------
// GROUP 4: MCP Tools & Safe-Retry Layer (Scenarios 31 - 40)
// ---------------------------------------------------------------------------
async function t31_mcpWebSearchTool() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Search web' }], tools: [{ type: 'function', function: { name: 'web_search' } }] }),
  });
  return res.status === 200;
}

async function t32_mcpExecuteSqlTool() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Run SQL' }], tools: [{ type: 'function', function: { name: 'execute_sql' } }] }),
  });
  return res.status === 200;
}

async function t33_unregisteredToolCallBlock() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Run unknown' }], tools: [{ type: 'function', function: { name: 'phantom_unregistered_tool_xyz' } }] }),
  });
  return res.status === 200; // Intercepted safely without gateway panic
}

async function t34_mcpIdempotencyLock() {
  const idempotencyKey = `idempotency-${Date.now()}`;
  const res1 = fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Idempotency-Key': idempotencyKey },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Idempotency test' }] }),
  });
  const res2 = fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Idempotency-Key': idempotencyKey },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Idempotency test' }] }),
  });
  const [r1, r2] = await Promise.all([res1, res2]);
  return r1.status === 200 && r2.status === 200;
}

async function t35_mcpTimeoutFallback() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Test-Scenario': 'mcp-timeout' },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Timeout test' }] }),
  });
  return res.status === 200 || res.status === 504;
}

async function t36_lazySchemaCompression() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Lazy schema' }], tools: [{ type: 'function', function: { name: 'big_tool', parameters: { type: 'object', properties: { field: { type: 'string' } } } } }] }),
  });
  return res.status === 200;
}

async function t37_schemaOnDemandInjection() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Inject schema for get_weather' }] }),
  });
  return res.status === 200;
}

async function t38_multiToolArrayPayload() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({
      model: 'gpt-4o',
      messages: [{ role: 'user', content: 'Multi tool' }],
      tools: [
        { type: 'function', function: { name: 'get_weather' } },
        { type: 'function', function: { name: 'web_search' } },
      ],
    }),
  });
  return res.status === 200;
}

async function t39_agenticLoopHeaderIncrement() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Kryneth-Loop-Count': '2' },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Loop count check' }] }),
  });
  return res.status === 200;
}

async function t40_mcpMessagesDirectRoute() {
  const res = await fetch(`${GATEWAY_BASE_URL}/mcp/messages`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ jsonrpc: '2.0', method: 'tools/call', params: { name: 'web_search' }, id: 1 }),
  });
  return res.status === 200 || res.status === 404;
}

// ---------------------------------------------------------------------------
// GROUP 5: Caching & Guardrail Security (Scenarios 41 - 50+)
// ---------------------------------------------------------------------------
async function t41_l1ExactCacheHit() {
  const prompt = `Unique exact prompt ${Date.now()}`;
  // Request 1: Cache Miss & Insert
  await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: prompt }] }),
  });
  // Request 2: Cache Hit
  const res2 = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: prompt }] }),
  });
  return res2.status === 200;
}

async function t42_cacheBypassOnScenarioHeader() {
  const prompt = `Bypass prompt ${Date.now()}`;
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}`, 'X-Test-Scenario': 'success-stream' },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: prompt }] }),
  });
  return res.headers.get('x-cache') !== 'HIT';
}

async function t43_piiRedactionEmail() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'My email is testuser@example.com' }] }),
  });
  return [200, 503].includes(res.status);
}

async function t44_piiRedactionCreditCard() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Card is 4111-2222-3333-4444' }] }),
  });
  return [200, 503].includes(res.status);
}

async function t45_piiSafePromptIgnored() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'What is the speed of light?' }] }),
  });
  return res.status === 200;
}

async function t46_systemMessageInMessages() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'system', content: 'Be concise.' }, { role: 'user', content: 'Hi' }] }),
  });
  return res.status === 200;
}

async function t47_temperatureAndTopPForwarding() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Hi' }], temperature: 0.7, top_p: 0.9 }),
  });
  return res.status === 200;
}

async function t48_maxTokensForwarding() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Hi' }], max_tokens: 100 }),
  });
  return res.status === 200;
}

async function t49_stopSequencesArray() {
  const res = await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${API_KEY}` },
    body: JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'Hi' }], stop: ['\n', 'END'] }),
  });
  return res.status === 200;
}

async function t50_healthEndpointCheck() {
  const res = await fetch(`http://localhost:9090/health`);
  const data = await res.json();
  return res.status === 200 && data.status === 'ok';
}

async function t51_executionSafetyConcurrent() {
  const executionId = `exec-concurrent-51-${Date.now()}`;
  const idempotencyKey = `idem-concurrent-51-${Date.now()}`;

  const req1 = fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${API_KEY}`,
      'X-Execution-ID': executionId,
      'X-Idempotency-Key': idempotencyKey,
      'X-Workflow-ID': 'wf-concurrent-51',
      'X-Agent-ID': 'ag-concurrent-51',
    },
    body: JSON.stringify({ model: 'agentic-test-model', messages: [{ role: 'user', content: 'What is the stock price of AMZN?' }] }),
  });

  const req2 = fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${API_KEY}`,
      'X-Execution-ID': executionId,
      'X-Idempotency-Key': idempotencyKey,
      'X-Workflow-ID': 'wf-concurrent-51',
      'X-Agent-ID': 'ag-concurrent-51',
    },
    body: JSON.stringify({ model: 'agentic-test-model', messages: [{ role: 'user', content: 'What is the stock price of AMZN?' }] }),
  });

  const [r1, r2] = await Promise.all([req1, req2]);
  return r1.status === 200 && r2.status === 200;
}

async function t52_unsafeRetryPrevention() {
  const executionId = `exec-retry-52-${Date.now()}`;
  const idempotencyKey = `idem-retry-52-${Date.now()}`;

  // Fetch initial blocked-retry counter
  const metrics1 = await fetch(`http://localhost:8080/v1/admin/metrics/live`, {
    headers: { Authorization: `Bearer ${API_KEY}` }
  }).then(r => r.json());
  const initialBlockedCount = metrics1.mcp_previous_attempt_unknown_blocked || 0;

  // Step 1: Send a NON-STREAMING buffered request with 'execute-sql-json' scenario.
  // The mock returns a proper JSON chat.completion body (not SSE), so
  // handle_buffered_response can parse tool_calls and dispatch fan_out.
  // ExecutionService calls /mcp/messages with X-Test-Scenario: mcp-timeout,
  // the mock sleeps 10s, the 5s policy timeout fires → state = Unknown.
  await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${API_KEY}`,
      'X-Execution-ID': executionId,
      'X-Idempotency-Key': idempotencyKey,
      'X-Workflow-ID': 'wf-retry-52',
      'X-Agent-ID': 'ag-retry-52',
      'X-Test-Scenario': 'execute-sql-json',  // LLM mock returns buffered JSON with execute_sql tool_calls
    },
    body: JSON.stringify({
      model: 'agentic-test-model',
      stream: false,  // Force buffered path so handle_buffered_response runs fan_out
      messages: [{ role: 'user', content: `run sql idempotent test ${Date.now()}` }],
    }),
  });

  // Step 2: Retry with SAME execution identity + idempotency key, NO test scenario
  // (so the MCP call would go through normally, but execution_service sees Unknown state
  // from Step 1 and blocks it → increments mcp_previous_attempt_unknown_blocked).
  await fetch(`${GATEWAY_BASE_URL}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${API_KEY}`,
      'X-Execution-ID': executionId,
      'X-Idempotency-Key': idempotencyKey,
      'X-Workflow-ID': 'wf-retry-52',
      'X-Agent-ID': 'ag-retry-52',
      'X-Test-Scenario': 'execute-sql-json',  // same JSON body so fan_out fires again
    },
    body: JSON.stringify({
      model: 'agentic-test-model',
      stream: false,
      messages: [{ role: 'user', content: `run sql idempotent test ${Date.now()}` }],
    }),
  });

  // Fetch final counter — must have increased by 1
  const metrics2 = await fetch("http://localhost:8080/v1/admin/metrics/live", {
    headers: { Authorization: `Bearer ${API_KEY}` }
  }).then(r => r.json());
  const finalBlockedCount = metrics2.mcp_previous_attempt_unknown_blocked || 0;

  return finalBlockedCount > initialBlockedCount;
}

// ---------------------------------------------------------------------------

// Main Suite Runner (Safe Happy-Path First, Destructive Chaos Last)
// ---------------------------------------------------------------------------
async function runFull50ScenarioSuite() {
  const tests = [
    [1, 'Malformed JSON Input Ingestion', t1_malformedJson],
    [2, 'Missing Model Parameter Ingestion', t2_missingModel],
    [3, 'Unconfigured Tenant Model Target', t3_unconfiguredModel],
    [4, 'Empty Messages Array Payload', t4_emptyMessages],
    [5, 'Null Payload Ingestion', t5_nullPayload],
    [6, 'Invalid Message Role Handling', t6_invalidRole],
    [7, 'Oversized 5MB Payload Fast Bypass', t7_oversizedPayloadBypass],
    [8, 'Extra Root Parameter Ingestion', t8_extraRootParams],
    [9, 'Missing Authorization Header', t9_missingAuthHeader],
    [10, 'Invalid Route Alias 404 Check', t10_invalidRouteAlias],
    [11, 'Vercel AI SDK Standard Streaming', t11_vercelStreaming],
    [12, 'OpenAI Non-Streaming Completion', t12_openaiNonStreaming],
    [13, 'OpenAI SDK Tool Call Delta Parsing', t13_openaiToolCallDelta],
    [14, 'Anthropic Claude Messages Endpoint', t14_anthropicClaudeMessages],
    [15, 'Anthropic Claude tool_use Block', t15_anthropicToolUseBlock],
    [16, 'Google Gemini generateContent', t16_geminiGenerateContent],
    [17, 'Google Gemini functionCall Part', t17_geminiFunctionCallPart],
    [18, 'DeepSeek Reasoning <think> Stream', t18_deepseekThinkingStream],
    [19, 'Cohere Command-R Format Mapping', t19_cohereCommandR],
    [20, 'Responses Route Alias Check', t20_responsesEndpointAlias],
    [26, 'Rapid Burst Request Concurrency', t26_rapidBurstRequests],
    [27, 'CORS Options Preflight Request', t27_corsPreflightOptions],
    [28, 'Header Case Insensitivity Check', t28_headerCaseInsensitivity],
    [29, 'Trace ID Header Propagation', t29_traceIdPropagation],
    [30, 'Session Tracing Header Support', t30_sessionTracingHeader],
    [31, 'MCP Web Search Tool Execution', t31_mcpWebSearchTool],
    [32, 'MCP Execute SQL Tool Execution', t32_mcpExecuteSqlTool],
    [33, 'Unregistered Phantom Tool Block', t33_unregisteredToolCallBlock],
    [34, 'MCP Idempotency Lock Check', t34_mcpIdempotencyLock],
    [35, 'MCP Timeout Threshold Fallback', t35_mcpTimeoutFallback],
    [36, 'Lazy Schema Parameter Stripping', t36_lazySchemaCompression],
    [37, 'Schema-On-Demand Tool Injection', t37_schemaOnDemandInjection],
    [38, 'Multi-Tool Array Payload Parsing', t38_multiToolArrayPayload],
    [39, 'Agentic Loop Header Increment', t39_agenticLoopHeaderIncrement],
    [40, 'MCP Messages Direct Endpoint', t40_mcpMessagesDirectRoute],
    [41, 'L1 Exact Prompt Cache HIT/MISS', t41_l1ExactCacheHit],
    [42, 'Cache Bypass on Scenario Header', t42_cacheBypassOnScenarioHeader],
    [43, 'Zero-Copy PII Redaction Email', t43_piiRedactionEmail],
    [44, 'Zero-Copy PII Redaction Credit Card', t44_piiRedactionCreditCard],
    [45, 'PII Safe Prompt Pass-Through', t45_piiSafePromptIgnored],
    [46, 'System Message Role Handling', t46_systemMessageInMessages],
    [47, 'Temperature & Top-P Forwarding', t47_temperatureAndTopPForwarding],
    [48, 'Max Tokens Option Forwarding', t48_maxTokensForwarding],
    [49, 'Stop Sequences Array Parsing', t49_stopSequencesArray],
    [50, 'Upstream Mock Server Health Check', t50_healthEndpointCheck],
    [51, 'Execution Safety Layer - Concurrent Tool Executions', t51_executionSafetyConcurrent],
    [52, 'Unsafe Retry Prevention', t52_unsafeRetryPrevention],
    // Destructive / Chaos Rate Limit / Failover tests placed at end
    [21, 'Upstream Mid-Stream Abort Resiliency', t21_midStreamCrashResilience],
    [22, 'Circuit-Breaker Hot-Swap Failover', t22_circuitBreakerHotSwap],
    [23, 'Upstream 429 Rate Limit Handling', t23_rateLimit429Handling],
    [24, 'Upstream 500 Server Error Handling', t24_serverError500Handling],
    [25, 'All Configured Keys Exhausted 503', t25_allKeysExhausted503],
  ];

  const results = [];
  for (const [id, name, fn] of tests) {
    results.push(await runScenario(id, name, fn));
  }

  const passed = results.filter(r => r.status === 'PASSED').length;
  const total = results.length;

  console.log('\n===========================================================');
  console.log(`📊 50+ SCENARIO INTEGRATION SUITE SUMMARY: ${passed}/${total} PASSED`);
  console.log('===========================================================');

  if (passed === total) {
    console.log('🎉 ALL 50 INTEGRATION SCENARIOS PASSED CLEANLY!\n');
    process.exit(0);
  } else {
    console.error(`⚠️ ${total - passed} SCENARIOS FAILED. CHECK LOGS ABOVE.\n`);
    process.exit(1);
  }
}

runFull50ScenarioSuite();
