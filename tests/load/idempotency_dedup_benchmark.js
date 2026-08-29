import http from 'k6/http';
import { check } from 'k6';

export const options = {
  vus: 50,
  duration: '20s',
  thresholds: {
    http_req_duration: ['p(95)<100'],
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

export default function () {
  const executionId = `exec-bench-${__VU}`;
  const idempotencyKey = `idem-bench-${__VU}`;

  const res = http.post(
    `${BASE_URL}/v1/chat/completions`,
    JSON.stringify({
      model: 'mock-model',
      messages: [{ role: 'user', content: 'Execute idempotent tool execution benchmark' }],
      stream: false,
    }),
    {
      headers: Object.assign({}, HEADERS, {
        'X-Workflow-ID': 'wf-idempotency-bench',
        'X-Agent-ID': 'ag-idempotency-bench',
        'X-Execution-ID': executionId,
        'X-Idempotency-Key': idempotencyKey,
      }),
    }
  );

  check(res, {
    'status is 200 or retry blocked': (r) => r.status === 200 || r.status === 400 || r.status === 409,
  });
}
