import { defineConfig } from "@playwright/test";

const databaseUrl =
  process.env.DATABASE_URL ??
  "postgres://postgres:postgres@127.0.0.1:5432/flight_vis_test";

const apiEnv = {
  ...process.env,
  DATABASE_URL: databaseUrl,
  FLIGHT_CLUB_SLUG: "demo",
  PORT: "8080",
};

export default defineConfig({
  testDir: "e2e",
  timeout: 30_000,
  projects: [
    {
      name: "dev-ui",
      testIgnore: /csp-prod\.spec\.ts/,
      use: {
        baseURL: "http://127.0.0.1:5173",
      },
    },
    {
      name: "prod-csp",
      testMatch: /csp-prod\.spec\.ts/,
      use: {
        baseURL: "http://127.0.0.1:8080",
      },
    },
  ],
  webServer: [
    {
      command:
        "VITE_E2E_SKIP_GLOBE=1 npm run build && cargo +1.98.0 run --release --manifest-path server-rs/Cargo.toml",
      url: "http://127.0.0.1:8080/api/health",
      env: apiEnv,
      timeout: 300_000,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: "VITE_E2E_SKIP_GLOBE=1 npm run dev -- --host 127.0.0.1 --port 5173",
      url: "http://127.0.0.1:5173",
      env: {
        ...process.env,
        VITE_E2E_SKIP_GLOBE: "1",
      },
      reuseExistingServer: !process.env.CI,
    },
  ],
});