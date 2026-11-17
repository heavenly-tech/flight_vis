import { chromium, devices } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import path from "node:path";

const base = process.env.SCREENSHOT_BASE ?? "http://127.0.0.1:8080";
const outDir = "/opt/cursor/artifacts/club-screenshots";

async function login(page, email) {
  const res = await page.request.post(`${base}/api/club/login`, {
    data: { email, password: "password12" },
  });
  if (!res.ok()) throw new Error(`login failed for ${email}: ${res.status()}`);
}

const shots = [
  { name: "club-login", path: "/club-login.html", auth: null },
  { name: "club-admin-types", path: "/club-admin-types.html", auth: "admin@demo.test" },
  { name: "club-admin-tails", path: "/club-admin-tails.html", auth: "admin@demo.test" },
  { name: "club-admin-pilots", path: "/club-admin-pilots.html", auth: "admin@demo.test" },
  { name: "club-admin-checkouts", path: "/club-admin-checkouts.html", auth: "admin@demo.test" },
  { name: "club-admin-map", path: "/club-admin-map.html", auth: "admin@demo.test" },
  { name: "my-flights", path: "/club-pilot.html", auth: "pilot@demo.test" },
  { name: "club-map", path: "/club-map.html", auth: "pilot@demo.test" },
  { name: "public-map", path: "/public-map.html", auth: null },
  { name: "app-download", path: "/app.html", auth: null },
];

async function capture() {
  await mkdir(outDir, { recursive: true });
  const browser = await chromium.launch();
  const ctxDesktop = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const ctxMobile = await browser.newContext({
    ...devices["Pixel 5"],
    viewport: { width: 390, height: 844 },
  });

  let flightPath = "/club-flight.html?id=demo";
  try {
    const page = await ctxDesktop.newPage();
    await login(page, "pilot@demo.test");
    const res = await page.request.get(`${base}/api/me/flights`);
    const data = await res.json();
    const id = data.flights?.[0]?.id;
    if (id) flightPath = `/club-flight.html?id=${encodeURIComponent(id)}`;
    await page.close();
  } catch {
    /* keep default */
  }

  const all = [...shots, { name: "club-flight", path: flightPath, auth: "pilot@demo.test" }];

  for (const spec of all) {
    for (const [label, ctx] of [
      ["desktop", ctxDesktop],
      ["mobile-390", ctxMobile],
    ]) {
      const page = await ctx.newPage();
      if (spec.auth) await login(page, spec.auth);
      await page.goto(`${base}${spec.path}`);
      await page.waitForTimeout(800);
      await page.screenshot({
        path: path.join(outDir, `${spec.name}-${label}.png`),
        fullPage: true,
      });
      await page.close();
    }
  }

  await browser.close();
  console.log(`Wrote screenshots to ${outDir}`);
}

capture().catch((err) => {
  console.error(err);
  process.exit(1);
});