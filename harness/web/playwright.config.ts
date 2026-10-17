import { defineConfig } from '@playwright/test';
import { FIXTURE_URL } from './ports';

export default defineConfig({
  testDir: '.',
  testMatch: '*.spec.ts',
  use: {
    baseURL: FIXTURE_URL,
  },
  webServer: {
    // Both harness servers (fixture + empty) come up in one process.
    command: 'cargo run -p mosquito-core --example harness_server',
    url: FIXTURE_URL,
    // Local development convenience: skip the build when it's already
    // listening (`yarn harness:web:server`).
    reuseExistingServer: true,
    timeout: 240_000,
  },
});
