import http from 'k6/http';
import { check, sleep } from 'k6';

// k6 options configuration
export const options = {
  vus: 100,
  duration: '30s',
  thresholds: {
    http_req_failed: ['rate<0.15'], // Expect up to 10-15% of requests to fail due to loop blocks (intentional 429s)
  },
};

const BASE_URL = 'http://localhost:8080';
const HEADERS = {
  'Content-Type': 'application/json',
  'Authorization': 'Bearer re_live_dev_123',
  'x-tenant-id': '00000000-0000-0000-0000-000000000000',
  'x-kryneth-model': 'mock-model',
};

// Static cache hit payload
const CACHE_HIT_PAYLOAD = JSON.stringify({
  model: 'mock-model',
  messages: [{ role: 'user', content: 'What is the capital of France?' }],
  stream: false,
});

// Setup: seed the cache hit payload
export function setup() {
  console.log('🌱 Seeding Cache Hit payload...');
  http.post(`${BASE_URL}/v1/chat/completions`, CACHE_HIT_PAYLOAD, { headers: HEADERS });
}

export default function () {
  const rand = Math.random();
  
  if (rand < 0.60) {
    // --- 1. Cache Hit (60% weight) ---
    // Uses a unique session ID per VU to keep metrics separate, but the same body to hit the cache
    const headers = Object.assign({ 'x-session-id': `session-hit-vu-${__VU}` }, HEADERS);
    const res = http.post(`${BASE_URL}/v1/chat/completions`, CACHE_HIT_PAYLOAD, { headers: headers });
    
    check(res, {
      'cache hit status is 200': (r) => r.status === 200,
      'cache hit header present': (r) => r.headers['X-Cache'] === 'HIT' || r.headers['x-cache'] === 'HIT',
    });
    
  } else if (rand < 0.90) {
    // --- 2. Cache Miss / New Request (30% weight) ---
    // Unique body per iteration ensures it bypasses the cache and calls the mock upstream
    const payload = JSON.stringify({
      model: 'mock-model',
      messages: [{ role: 'user', content: `Random prompt number: ${__ITER}-${Math.floor(Math.random() * 1000000)}` }],
      stream: false,
    });
    
    const headers = Object.assign({ 'x-session-id': `session-miss-vu-${__VU}` }, HEADERS);
    const res = http.post(`${BASE_URL}/v1/chat/completions`, payload, { headers: headers });
    
    check(res, {
      'cache miss status is 200': (r) => r.status === 200,
      'cache miss header present': (r) => r.headers['X-Cache'] === 'MISS' || r.headers['x-cache'] === 'MISS',
    });
    
  } else {
    // --- 3. Looping Agent (10% weight) ---
    // Sends repeated identical tool call payloads on the same session ID.
    // This will trip the loop trap after 5 iterations for this VU.
    const payload = JSON.stringify({
      model: 'mock-model',
      messages: [
        { role: 'user', content: 'Do work' },
        { 
          role: 'assistant', 
          content: null, 
          tool_calls: [
            { 
              id: 'call_loop', 
              type: 'function', 
              function: { name: 'get_weather', arguments: '{"city":"Tokyo"}' } 
            }
          ] 
        }
      ],
      stream: false,
    });
    
    const headers = Object.assign({ 'x-session-id': `session-loop-vu-${__VU}` }, HEADERS);
    const res = http.post(`${BASE_URL}/v1/chat/completions`, payload, { headers: headers });
    
    // Depending on how many times this VU has hit this branch, it will either be 200 or 429.
    check(res, {
      'loop branch returns 200 or 429': (r) => r.status === 200 || r.status === 429,
    });
  }

  // Brief sleep to space out requests
  sleep(0.05);
}
