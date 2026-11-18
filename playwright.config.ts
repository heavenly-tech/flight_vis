import { defineConfig } from "@playwright/test";

const databaseUrl =
  process.env.DATABASE_URL ??
  "postgres://postgres:postgres@127.0.0.1:5432/flight_vis_test";

export default defineConfig({
  testDir: "e2e",
  timeout: 30_000,
  use: {
    baseURL: "http://127.0.0.1:5173",
  },
  webServer: [
    {
      command:
        "npm run build && cargo +1.98.0 run --manifest-path server-rs/Cargo.toml",
      url: "http://127.0.0.1:8080/api/health",
      env: {
        ...process.env,
        DATABASE_URL: databaseUrl,
        FLIGHT_CLUB_SLUG: "demo",
        PORT: "8080",
      },
      timeout: 120_000,
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