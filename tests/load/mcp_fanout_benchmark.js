import http from 'k6/http';
import { check } from 'k6';

export const options = {
  vus: 30, // Lower VUs because each VU triggers 10 parallel requests
  duration: '20s',
  thresholds: {
    http_req_duration: ['p(95)<150'], // Fanout takes slightly longer due to orchestration
    http_req_failed: ['rate<0.01'],
  },
};

const BASE_URL = 'http://localhost:8080';
const HEADERS = {
  'Content-Type': 'application/json',
  'Authorization': 'Bearer re_live_dev_123',
  'x-tenant-id': '00000000-0000-0000-0000-000000000000',
  'x-kryneth-model': 'mock-model',
};

// Generates a mock MCP tool call request
function createFanoutRequest(id, executionId, idempotencyKey) {
  const headers = Object.assign({}, HEADERS, {
    'X-Workflow-ID': 'wf-bench',
    'X-Agent-ID': 'ag-bench',
    'X-Execution-ID': executionId,
    'X-Idempotency-Key': `${idempotencyKey}-${id}`,
  });
  return {
    method: 'POST',
    url: `${BASE_URL}/v1/chat/completions`,
    body: JSON.stringify({
      model: 'mock-model',
      messages: [{ role: 'user', content: `Execute MCP tool function ${id} for iter ${__ITER}` }],
      stream: false,
    }),
    params: { headers: headers },
  };
}

export default function () {
  const executionId = `exec-${__VU}-${__ITER}-${Math.random()}`;
  const idempotencyKey = `idem-${__VU}-${__ITER}-${Math.random()}`;

  // Mock a scenario where 1 parent request triggers 10 parallel tool calls (fan-out)
  const requests = [];
  for (let i = 0; i < 10; i++) {
    requests.push(createFanoutRequest(i, executionId, idempotencyKey));
  }

  // http.batch executes the requests in parallel
  const responses = http.batch(requests);

  // Validate that all parallel tool calls succeeded
  let allSuccessful = true;
  for (const res of responses) {
    if (res.status !== 200) {
      allSuccessful = false;
    }
  }

  check(responses, {
    'all fanout requests status 200': () => allSuccessful,
  });
}
