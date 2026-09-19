import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile, mkdir } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { COOKIE, createAuth, parseCookie } from "./auth.mjs";
import { createApp, seed } from "./app.mjs";

const TRACK = `<?xml version="1.0"?>
<gpx version="1.1"><trk><name>Test</name><trkseg>
<trkpt lat="-33.4" lon="-70.6"><ele>800</ele><time>2024-01-01T12:00:00Z</time></trkpt>
<trkpt lat="-33.41" lon="-70.61"><ele>820</ele><time>2024-01-01T12:01:00Z</time></trkpt>
</trkseg></trk></gpx>`;

function setCookies(res) {
  if (typeof res.headers.getSetCookie === "function") return res.headers.getSetCookie();
  const single = res.headers.get("set-cookie");
  return single ? [single] : [];
}

function cookieHeader(res) {
  return setCookies(res)
    .map((part) => part.split(";", 1)[0])
    .filter(Boolean)
    .join("; ");
}

describe("flight API auth", () => {
  let dataDir;
  let distDir;
  let app;
  const auth = createAuth({ FLIGHT_VIS_PASSWORD: "secret" });

  before(async () => {
    dataDir = await mkdtemp(path.join(os.tmpdir(), "flight-vis-"));
    distDir = path.join(dataDir, "dist");
    await mkdir(distDir, { recursive: true });
    await writeFile(path.join(distDir, "index.html"), "<html>app</html>");
    await writeFile(path.join(distDir, "login.html"), "<html>login</html>");
    await seed(dataDir, { samples: [] });
    app = createApp({ dataDir, distDir, auth });
  });

  after(async () => {
    await rm(dataDir, { recursive: true, force: true });
  });

  async function login(password = "secret") {
    const res = await app.request("/login", {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({ password }),
    });
    return res;
  }

  it("keeps health and slug reads public", async () => {
    const seeded = await app.request("/api/health");
    assert.equal(seeded.status, 200);

    const put = await app.request("/api/flights/demo", {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
        Cookie: `${COOKIE}=${auth.mint()}`,
      },
      body: JSON.stringify({ name: "Demo", fileName: "demo.gpx", track: TRACK, tags: [] }),
    });
    assert.equal(put.status, 200);

    const get = await app.request("/api/flights/demo");
    assert.equal(get.status, 200);
    const body = await get.json();
    assert.equal(body.slug, "demo");
    assert.match(body.track, /<gpx/);
  });

  it("rejects unauthenticated upload and flight list", async () => {
    const list = await app.request("/api/flights");
    assert.equal(list.status, 401);

    const put = await app.request("/api/flights/sneak", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Nope", fileName: "nope.gpx", track: TRACK }),
    });
    assert.equal(put.status, 401);
    assert.equal((await put.json()).error, "unauthorized");
  });

  it("logs in with the env password and then allows save + list", async () => {
    const wrong = await login("wrong");
    assert.equal(wrong.status, 401);

    const ok = await login("secret");
    assert.equal(ok.status, 200);
    const cookie = cookieHeader(ok);
    assert.match(cookie, new RegExp(`^${COOKIE}=`));
    assert.equal(Boolean(parseCookie(cookie)), true);

    const me = await app.request("/api/auth/me", { headers: { Cookie: cookie } });
    assert.deepEqual(await me.json(), { authenticated: true, configured: true });

    const put = await app.request("/api/flights/saved", {
      method: "PUT",
      headers: { "Content-Type": "application/json", Cookie: cookie },
      body: JSON.stringify({ name: "Saved", fileName: "saved.gpx", track: TRACK, tags: [] }),
    });
    assert.equal(put.status, 200);

    const list = await app.request("/api/flights", { headers: { Cookie: cookie } });
    assert.equal(list.status, 200);
    const items = await list.json();
    assert.equal(items.some((item) => item.slug === "saved"), true);
  });

  it("locks writes when no password is configured", async () => {
    const locked = createApp({
      dataDir,
      distDir,
      auth: createAuth({}),
    });
    const put = await locked.request("/api/flights/locked", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Locked", fileName: "x.gpx", track: TRACK }),
    });
    assert.equal(put.status, 401);
    assert.equal((await put.json()).error, "locked");

    const form = await locked.request("/login", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: "password=anything",
    });
    assert.equal(form.status, 302);
    assert.equal(form.headers.get("location"), "/login?locked=1");
  });

  it("serves login and clears the session on logout", async () => {
    const page = await app.request("/login");
    assert.equal(page.status, 200);
    assert.match(await page.text(), /login/i);

    const ok = await login("secret");
    const cookie = cookieHeader(ok);
    const out = await app.request("/logout", {
      method: "POST",
      headers: { Accept: "application/json", Cookie: cookie },
    });
    assert.equal(out.status, 200);
    assert.match(setCookies(out).join("\n"), /Max-Age=0/);

    const me = await app.request("/api/auth/me");
    assert.deepEqual(await me.json(), { authenticated: false, configured: true });
  });
});
