#!/usr/bin/env node
/**
 * Capture Spanish (es) UI screenshots for PR evidence; one English sample.
 * Run with API + Vite up, or via Playwright webServer (see playwright.config).
 */
import { chromium } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";

const outDir = process.env.SCREENSHOT_DIR ?? "/opt/cursor/artifacts/i18n-screens";
const base = process.env.PW_BASE ?? "http://127.0.0.1:5173";

const pages = [
  { name: "club-login-es", url: "/club-login.html", locale: "es" },
  { name: "club-register-es", url: "/club-register.html", locale: "es" },
  { name: "club-pilot-es", url: "/club-pilot.html", locale: "es" },
  { name: "club-map-es", url: "/club-map.html", locale: "es" },
  { name: "public-map-es", url: "/public-map.html", locale: "es" },
  { name: "club-admin-types-es", url: "/club-admin-types.html", locale: "es" },
  { name: "club-admin-tails-es", url: "/club-admin-tails.html", locale: "es" },
  { name: "club-admin-pilots-es", url: "/club-admin-pilots.html", locale: "es" },
  { name: "club-admin-checkouts-es", url: "/club-admin-checkouts.html", locale: "es" },
  { name: "club-admin-map-es", url: "/club-admin-map.html", locale: "es" },
  { name: "club-admin-kiosk-layout-es", url: "/club-admin-kiosk-layout.html", locale: "es" },
  { name: "club-screen-es", url: "/club-screen.html", locale: "es" },
  { name: "club-checkin-expired-es", url: "/club-checkin-expired.html", locale: "es" },
  { name: "app-es", url: "/app.html", locale: "es" },
  { name: "club-login-en", url: "/club-login.html", locale: "en" },
];

fs.mkdirSync(outDir, { recursive: true });

const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
const page = await context.newPage();

for (const entry of pages) {
  await context.addCookies([
    {
      name: "fv_locale",
      value: entry.locale,
      domain: "127.0.0.1",
      path: "/",
    },
  ]);
  await page.goto(`${base}${entry.url}`, { waitUntil: "networkidle" });
  await page.waitForTimeout(400);
  const file = path.join(outDir, `${entry.name}.png`);
  await page.screenshot({ path: file, fullPage: true });
  console.log("wrote", file);
}

await browser.close();