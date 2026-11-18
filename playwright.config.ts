import { defineConfig } from "@playwright/test";

const databaseUrl =
  process.env.DATABASE_URL ??
  "postgres://postgres:postgres@127.0.0.1:5432/flight_vis_test";

const e2eApiPort = process.env.PORT ?? process.env.FLIGHT_VIS_E2E_API_PORT ?? "8080";
const e2eCspPort = process.env.E2E_CSP_PORT ?? "19090";

const apiEnv = {
  ...process.env,
  DATABASE_URL: databaseUrl,
  FLIGHT_CLUB_SLUG: "demo",
  PORT: e2eApiPort,
};

export default defineConfig({
  testDir: "e2e",
  timeout: 30_000,
  projects: [
    {
      name: "dev-ui",
      testIgnore: [/csp-prod\.spec\.ts/, /globe-csp\.spec\.ts/],
      use: {
        baseURL: "http://127.0.0.1:5173",
      },
    },
    {
      name: "prod-csp",
      testMatch: /csp-prod\.spec\.ts/,
      timeout: 120_000,
      use: {
        baseURL: `http://127.0.0.1:${e2eApiPort}`,
      },
    },
    {
      name: "globe-csp",
      testMatch: /globe-csp\.spec\.ts/,
      fullyParallel: false,
      workers: 1,
      timeout: 120_000,
      use: {
        baseURL: `http://127.0.0.1:${e2eCspPort}`,
      },
    },
  ],
  webServer: [
    {
      command:
        "npm run build && cargo +1.98.0 run --release --manifest-path server-rs/Cargo.toml",
      url: `http://127.0.0.1:${e2eApiPort}/api/health`,
      env: apiEnv,
      timeout: 300_000,
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
      command: `PORT=${e2eApiPort} VITE_E2E_SKIP_GLOBE=1 npm run dev -- --host 127.0.0.1 --port 5173`,
      url: "http://127.0.0.1:5173",
      env: {
        ...process.env,
        PORT: e2eApiPort,
        VITE_E2E_SKIP_GLOBE: "1",
      },
      reuseExistingServer: !process.env.CI,
    },
  ],
});