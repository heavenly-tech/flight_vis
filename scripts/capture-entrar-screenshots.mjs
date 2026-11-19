#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const outDir = path.join(root, "docs", "screenshots");
fs.mkdirSync(outDir, { recursive: true });

const base = process.env.E2E_BASE_URL ?? "http://127.0.0.1:5173";
const shots = [
  { name: "entrar-chooser-es", path: "/login", locale: "es" },
  { name: "entrar-login-es", path: "/login", locale: "es" },
  { name: "entrar-password-es", path: "/login?mode=password", locale: "es" },
];

const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
const page = await context.newPage();

for (const s of shots) {
  await page.goto(`${base}${s.path}`, { waitUntil: "networkidle" });
  if (s.locale) {
    await page.evaluate((loc) => {
      document.cookie = `fv_display_lang=${loc}; path=/`;
      location.reload();
    }, s.locale);
    await page.waitForLoadState("networkidle");
  }
  await page.screenshot({ path: path.join(outDir, `${s.name}.png`), fullPage: true });
}

await browser.close();
console.log(`wrote ${shots.length} screenshots to ${outDir}`);