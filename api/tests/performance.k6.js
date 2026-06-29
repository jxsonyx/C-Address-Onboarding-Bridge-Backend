import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.API_BASE_URL || 'http://localhost:3001';
const API_KEY = __ENV.API_KEY || 'benchmark-api-key';

export const options = {
  stages: [
    { duration: '20s', target: 20 },
    { duration: '40s', target: 20 },
    { duration: '20s', target: 0 },
  ],
  thresholds: {
    http_req_duration: ['p(95)<500'],
    http_req_failed: ['rate<0.01'],
    checks: ['rate>0.99'],
  },
};

export default function () {
  const healthRes = http.get(`${BASE_URL}/health/live`);
  check(healthRes, {
    'liveness status is 200': (response) => response.status === 200,
  });

  const quoteRes = http.get(
    `${BASE_URL}/api/v1/quote?sourceAsset=XLM&amount=1000000&targetAddress=CABCDEFGHIJKLMNOPQRSTUVWXYZ234567ABCDEFGHIJKLMNOPQRSTUVW`,
    { headers: { 'X-API-Key': API_KEY, Accept: 'application/json' } },
  );
  check(quoteRes, {
    'quote status is 200': (response) => response.status === 200,
    'quote includes expected fields': (response) => {
      try {
        const body = response.json();
        return Boolean(body.estimatedFee && body.expectedReceive && body.feeBps);
      } catch {
        return false;
      }
    },
  });

  sleep(1);
}
