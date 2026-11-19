import path from "node:path";
import type { IncomingMessage, ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin, type ProxyOptions } from "vitest/config";
import { viteStaticCopy } from "vite-plugin-static-copy";

const root = path.dirname(fileURLToPath(import.meta.url));
const cesiumSource = "node_modules/cesium/Build/Cesium";
const cesiumBaseUrl = "cesiumStatic";
const api = "http://127.0.0.1:8080";

function normalizeUrlPath(urlPath: string): string | null {
  const trimmed = urlPath.trim();
  if (!trimmed.startsWith("/")) return null;
  const parts: string[] = [];
  for (const part of trimmed.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") return null;
    parts.push(part);
  }
  if (!parts.length) return null;
  return `/${parts.join("/")}`;
}

const authProxy: Record<string, string | ProxyOptions> = {
  "/.well-known": api,
  "/c/": api,
  "/s/": api,
  "/api": api,
  "/signup": {
    target: api,
    bypass(req) {
      if (req.method === "GET" || req.method === "HEAD") {
        return "/signup.html";
      }
    },
  },
  "/logout": api,
  "/login": api,
  "/login/legacy": api,
  "/club-login.html": api,
};

function clubAdminHtmlGateMiddleware(
  apiOrigin: string,
):
  | ((req: IncomingMessage, res: ServerResponse, next: () => void) => void)
  | undefined {
  return (req, res, next) => {
    const method = req.method ?? "GET";
    const raw = req.url ?? "";
    const pathOnly = normalizeUrlPath(raw.split("?")[0] ?? "");
    if (
      (method !== "GET" && method !== "HEAD") ||
      !pathOnly ||
      !/^\/club-admin-[^/]+\.html$/.test(pathOnly)
    ) {
      next();
      return;
    }
    const search = raw.includes("?") ? raw.slice(raw.indexOf("?")) : "";
    const cookie = req.headers.cookie ?? "";
    const allowStaff = async (): Promise<boolean> => {
      const accountMe = await fetch(`${apiOrigin}/api/account/me`, {
        headers: { cookie },
      });
      if (accountMe.ok) {
        const data = (await accountMe.json()) as {
          activeMembershipId?: string | null;
          memberships?: Array<{ id: string; status: string; role?: string }>;
        };
        const list = data.memberships ?? [];
        const active = data.activeMembershipId
          ? list.find((m) => m.id === data.activeMembershipId)
          : undefined;
        const staff = (m: { status: string; role?: string }) =>
          m.status === "active" && (m.role === "admin" || m.role === "staff");
        if (active && staff(active)) {
          return true;
        }
        if (list.some(staff)) {
          return true;
        }
      }
      const clubMe = await fetch(`${apiOrigin}/api/club/me`, { headers: { cookie } });
      if (clubMe.ok) {
        const me = (await clubMe.json()) as { pilot?: { role?: string } | null };
        if (me.pilot?.role === "admin" || me.pilot?.role === "staff") {
          return true;
        }
      }
      return false;
    };
    void allowStaff()
      .then((ok) => {
        if (ok) {
          next();
          return;
        }
        res.statusCode = 302;
        res.setHeader(
          "Location",
          `/login?next=${encodeURIComponent(pathOnly + search)}`,
        );
        res.end();
      })
      .catch(() => {
        res.statusCode = 302;
        res.setHeader(
          "Location",
          `/login?next=${encodeURIComponent(pathOnly + search)}`,
        );
        res.end();
      });
  };
}

function clubAdminHtmlGatePlugin(apiOrigin: string): Plugin {
  const attach = (server: { middlewares: { use: (fn: ReturnType<typeof clubAdminHtmlGateMiddleware>) => void } }) => {
    const gate = clubAdminHtmlGateMiddleware(apiOrigin);
    if (gate) server.middlewares.use(gate);
  };
  return {
    name: "club-admin-html-gate",
    configureServer: attach,
    configurePreviewServer: attach,
  };
}

export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "jsdom",
  },
  base: "/",
  define: {
    CESIUM_BASE_URL: JSON.stringify(`/${cesiumBaseUrl}/`),
    "import.meta.env.VITE_E2E_SKIP_GLOBE": JSON.stringify(
      process.env.VITE_E2E_SKIP_GLOBE ?? "",
    ),
  },
  plugins: [
    clubAdminHtmlGatePlugin(process.env.FLIGHT_VIS_API_ORIGIN ?? api),
    viteStaticCopy({
      targets: [
        { src: "locales/*", dest: "locales" },
        { src: `${cesiumSource}/ThirdParty`, dest: cesiumBaseUrl },
        { src: `${cesiumSource}/Workers`, dest: cesiumBaseUrl },
        { src: `${cesiumSource}/Assets`, dest: cesiumBaseUrl },
        { src: `${cesiumSource}/Widgets`, dest: cesiumBaseUrl },
        { src: "assets/brand", dest: "assets/brand" },
      ],
    }),
  ],
  server: {
    host: true,
    port: 5173,
    proxy: authProxy,
  },
  preview: {
    host: true,
    port: 4173,
    proxy: authProxy,
  },
  build: {
    chunkSizeWarningLimit: 4000,
    rollupOptions: {
      input: {
        main: path.resolve(root, "index.html"),
        login: path.resolve(root, "login.html"),
        loginLegacy: path.resolve(root, "login-legacy.html"),
        accountClubs: path.resolve(root, "account-clubs.html"),
        account: path.resolve(root, "account.html"),
        app: path.resolve(root, "app.html"),
        clubPilot: path.resolve(root, "club-pilot.html"),
        clubFlight: path.resolve(root, "club-flight.html"),
        signup: path.resolve(root, "signup.html"),
        clubMap: path.resolve(root, "club-map.html"),
        publicMap: path.resolve(root, "public-map.html"),
        clubRegister: path.resolve(root, "club-register.html"),
        clubSignup: path.resolve(root, "club-signup.html"),
        clubSuspended: path.resolve(root, "club-suspended.html"),
        clubAdminTypes: path.resolve(root, "club-admin-types.html"),
        clubAdminTails: path.resolve(root, "club-admin-tails.html"),
        clubAdminPilots: path.resolve(root, "club-admin-pilots.html"),
        clubAdminCheckouts: path.resolve(root, "club-admin-checkouts.html"),
        clubAdminMap: path.resolve(root, "club-admin-map.html"),
        clubAdminKioskLayout: path.resolve(root, "club-admin-kiosk-layout.html"),
        clubAdminCheckinQr: path.resolve(root, "club-admin-checkin-qr.html"),
        clubScreen: path.resolve(root, "club-screen.html"),
        clubGlobeScreen: path.resolve(root, "club-globe-screen.html"),
        clubCheckinLanding: path.resolve(root, "club-checkin-landing.html"),
        clubCheckinExpired: path.resolve(root, "club-checkin-expired.html"),
        shareFlight: path.resolve(root, "share-flight.html"),
        e2eLive: path.resolve(root, "e2e-live.html"),
      },
    },
  },
});
