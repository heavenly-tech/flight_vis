import { defineConfig } from "@playwright/test";

const databaseUrl =
  process.env.DATABASE_URL ??
  "postgres://postgres:postgres@127.0.0.1:5432/flight_vis_test";

const e2eApiPort = process.env.PORT ?? process.env.FLIGHT_VIS_E2E_API_PORT ?? "8080";
const e2eCspPort = process.env.E2E_CSP_PORT ?? "19090";

export default defineConfig({
  testDir: "e2e",
  timeout: 30_000,
  use: {
    baseURL: "http://127.0.0.1:5173",
  },
  projects: [
    {
      name: "default",
      testIgnore: /globe-csp\.spec\.ts|auth-csp\.spec\.ts|admin-checkin-qr\.spec\.ts/,
    },
    {
      name: "globe-csp",
      testMatch: /(globe-csp|auth-csp)\.spec\.ts/,
      fullyParallel: false,
      workers: 1,
      use: { baseURL: `http://127.0.0.1:${e2eCspPort}` },
    },
    {
      name: "admin-checkin-qr",
      testMatch: /admin-checkin-qr\.spec\.ts/,
      fullyParallel: false,
      workers: 1,
    },
  ],
  webServer: [
    {
      command:
        "npm run build && cargo +1.98.0 run --manifest-path server-rs/Cargo.toml",
      url: `http://127.0.0.1:${e2eApiPort}/api/health`,
      env: {
        ...process.env,
        DATABASE_URL: databaseUrl,
        FLIGHT_CLUB_SLUG: "demo",
        FLIGHT_CLUB_SEED_DEMO: "1",
        PORT: e2eApiPort,
      },
      timeout: 120_000,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: "node scripts/e2e-csp-gateway.mjs",
      url: `http://127.0.0.1:${e2eCspPort}/`,
      env: {
        ...process.env,
        E2E_CSP_PORT: e2eCspPort,
        FLIGHT_VIS_API_ORIGIN: `http://127.0.0.1:${e2eApiPort}`,
      },
      timeout: 120_000,
      reuseExistingServer: !process.env.CI,
    },
    {
      command:
        "VITE_E2E_SKIP_GLOBE=1 npm run build && npm run preview -- --host 127.0.0.1 --port 5173",
      url: "http://127.0.0.1:5173",
      env: {
        ...process.env,
        VITE_E2E_SKIP_GLOBE: "1",
      },
      timeout: 180_000,
      reuseExistingServer: !process.env.CI,
    },
  ],
});