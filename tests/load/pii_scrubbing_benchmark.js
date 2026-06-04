import http from 'k6/http';
import { check } from 'k6';

// k6 options configuration
export const options = {
  vus: 50, // 50 concurrent users (regex & HTTP compliance call is more intensive)
  duration: '20s',
  thresholds: {
    http_req_failed: ['rate<0.01'],
    http_req_duration: ['p(95)<30'], // Expect < 30ms latency for local compliance roundtrip
  },
};

const BASE_URL = 'http://localhost:8080';
const HEADERS = {
  'Content-Type': 'application/json',
  'Authorization': 'Bearer re_live_dev_123',
  'x-tenant-id': '00000000-0000-0000-0000-000000000000',
  'x-kryneth-model': 'mock-model',
};

// Generates a request body with unique content to ensure cache misses
// and containing PII (email, phone, credit card) to trigger Kryneth PII engine.
function generatePIIPayload(iteration) {
  const email = `user_${iteration}_${Math.floor(Math.random() * 100000)}@testdomain.com`;
  const creditCard = '4111-2222-3333-4444';
  const phone = '202-555-0143';
  
  return JSON.stringify({
    model: 'mock-model',
    messages: [
      { 
        role: 'user', 
        content: `Hello, my email is ${email}. My credit card number is ${creditCard} and phone is ${phone}. Please process payment for txn #${iteration}.` 
      }
    ],
    stream: false,
  });
}

export default function (data) {
  // __ITER is a built-in k6 variable tracking the VU iteration count
  const payload = generatePIIPayload(__ITER);
  const res = http.post(`${BASE_URL}/v1/chat/completions`, payload, { headers: HEADERS });

  check(res, {
    'status is 200': (r) => r.status === 200,
    'response has mock text': (r) => r.body.includes('This is a mock response'),
  });
}
