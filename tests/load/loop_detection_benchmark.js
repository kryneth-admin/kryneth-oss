import http from 'k6/http';
import { check, sleep } from 'k6';

// k6 options configuration
export const options = {
  vus: 10, // 10 distinct agent sessions
  duration: '15s',
  thresholds: {
    // We expect some requests to fail (return 429) because we are deliberately triggering the Loop Trap!
    // So we don't assert http_req_failed here.
  },
};

const BASE_URL = 'http://localhost:8080';

// A static payload containing a tool call signature
const PAYLOAD = JSON.stringify({
  model: 'mock-model',
  messages: [
    {
      role: 'user',
      content: 'Run calculations'
    },
    {
      role: 'assistant',
      content: null,
      tool_calls: [
        {
          id: 'call_abc123',
          type: 'function',
          function: {
            name: 'calculate_math',
            arguments: '{"expression":"2+2"}'
          }
        }
      ]
    }
  ],
  stream: false,
});

export default function () {
  // Use the VU number to isolate sessions
  const sessionId = `session-benchmark-vu-${__VU}`;
  
  const headers = {
    'Content-Type': 'application/json',
    'Authorization': 'Bearer re_live_dev_123',
    'x-tenant-id': '00000000-0000-0000-0000-000000000000',
    'x-session-id': sessionId,
    'x-kryneth-model': 'mock-model',
  };

  const res = http.post(`${BASE_URL}/v1/chat/completions`, PAYLOAD, { headers: headers });

  // __ITER is the current iteration number for this VU.
  // The default limit for identical tool calls is 5.
  // Iterations 0-4 (first 5 calls) should succeed (status 200).
  // Iteration 5+ should be blocked (status 429).
  if (__ITER < 5) {
    check(res, {
      'initial calls succeed (status 200)': (r) => r.status === 200,
    });
  } else {
    check(res, {
      'subsequent calls blocked (status 429)': (r) => r.status === 429,
      'error response structure is valid': (r) => {
        const json = r.json();
        return json && json.error && json.error.message !== undefined;
      },
    });
  }

  // Sleep slightly to simulate agent thinking/round-trip delay
  sleep(0.1);
}
