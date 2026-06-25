import http from 'k6/http';
import { check, sleep } from 'k6';

// k6 options configuration
export const options = {
  vus: 1000, // 1000 concurrent users for extreme stress test
  duration: '30s', // run for 30 seconds
  thresholds: {
    // Assertions: fail-rate < 1%, 95% of cache hits should be < 5ms
    http_req_failed: ['rate<0.01'],
    http_req_duration: ['p(95)<60'],
  },
};

const BASE_URL = 'http://localhost:8080';
const HEADERS = {
  'Content-Type': 'application/json',
  'Authorization': 'Bearer re_live_dev_123', // Match key in gateway config
  'x-tenant-id': '00000000-0000-0000-0000-000000000000',
  'x-kryneth-model': 'mock-model',
};

const PAYLOAD = JSON.stringify({
  model: 'mock-model',
  messages: [{ role: 'user', content: 'What is the speed of light?' }],
  stream: false,
});

// Setup: seed the cache once before load test starts
export function setup() {
  console.log('🌱 Seeding Kryneth Cache with initial request...');
  const res = http.post(`${BASE_URL}/v1/chat/completions`, PAYLOAD, { headers: HEADERS });
  
  const success = check(res, {
    'setup request succeeded': (r) => r.status === 200,
  });

  if (!success) {
    throw new Error(`Failed to seed cache: HTTP ${res.status}. Is the gateway running on ${BASE_URL}?`);
  }
  
  console.log('✅ Cache seeded successfully. Starting benchmark...');
}

export default function () {
  const res = http.post(`${BASE_URL}/v1/chat/completions`, PAYLOAD, { headers: HEADERS });

  check(res, {
    'status is 200': (r) => r.status === 200,
    'cache hit header present': (r) => r.headers['X-Cache'] === 'HIT' || r.headers['x-cache'] === 'HIT',
  });
}
