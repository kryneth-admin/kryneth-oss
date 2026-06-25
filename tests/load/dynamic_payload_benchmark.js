import http from 'k6/http';
import { check } from 'k6';
import { randomString } from 'https://jslib.k6.io/k6-utils/1.2.0/index.js';

export const options = {
  vus: 50,
  duration: '20s',
  thresholds: {
    http_req_duration: ['p(95)<50'],
    http_req_failed: ['rate<0.01'],
  },
};

const BASE_URL = 'http://localhost:8080';
const HEADERS = {
  'Content-Type': 'application/json',
  'Authorization': 'Bearer re_live_dev_123',
  'x-tenant-id': '00000000-0000-0000-0000-000000000000',
};

// Pre-generate random strings to avoid CPU overhead during test
const STR_1KB = randomString(1024);
const STR_10KB = randomString(10240);
const STR_100KB = randomString(102400);

export default function () {
  // Randomly pick a payload size for this iteration
  const sizes = [STR_1KB, STR_10KB, STR_100KB];
  const selectedString = sizes[Math.floor(Math.random() * sizes.length)];
  
  // Add a random prefix to completely bypass L1 Exact Cache
  const uniquePrefix = `id_${__ITER}_${Math.random()}`;

  const payload = JSON.stringify({
    model: 'mock-model',
    messages: [
      { role: 'user', content: `${uniquePrefix} - Analyze this data: ${selectedString}` }
    ],
    stream: false,
  });

  const res = http.post(`${BASE_URL}/v1/chat/completions`, payload, { headers: HEADERS });

  check(res, {
    'status is 200': (r) => r.status === 200,
    'duration < 50ms': (r) => r.timings.duration < 50,
  });
}
