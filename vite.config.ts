import path from "node:path";
import type { IncomingMessage, ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin, type ProxyOptions } from "vitest/config";
import { viteStaticCopy } from "vite-plugin-static-copy";

const root = path.dirname(fileURLToPath(import.meta.url));
const cesiumSource = "node_modules/cesium/Build/Cesium";
const cesiumBaseUrl = "cesiumStatic";
const api = "http://127.0.0.1:8080";

const authProxy: Record<string, string | ProxyOptions> = {
  "/.well-known": api,
  "/c/": api,
  "/s/": api,
  "/api": api,
  "/logout": api,
  "/login": {
    target: api,
    bypass(req) {
      if (req.method === "GET" || req.method === "HEAD") {
        const query = req.url?.includes("?") ? req.url.slice(req.url.indexOf("?")) : "";
        return `/login.html${query}`;
      }
    },
  },
};

function clubAdminHtmlGateMiddleware(
  apiOrigin: string,
):
  | ((req: IncomingMessage, res: ServerResponse, next: () => void) => void)
  | undefined {
  return (req, res, next) => {
    const method = req.method ?? "GET";
    const raw = req.url ?? "";
    const pathOnly = raw.split("?")[0] ?? "";
    if ((method !== "GET" && method !== "HEAD") || !/^\/club-admin-[^/]+\.html$/.test(pathOnly)) {
      next();
      return;
    }
    const search = raw.includes("?") ? raw.slice(raw.indexOf("?")) : "";
    void fetch(`${apiOrigin}/api/club/me`, {
      headers: { cookie: req.headers.cookie ?? "" },
    })
      .then(async (meRes) => {
        if (meRes.ok) {
          const me = (await meRes.json()) as { pilot?: { role?: string } | null };
          if (me.pilot?.role === "admin") {
            next();
            return;
          }
        }
        res.statusCode = 302;
        res.setHeader(
          "Location",
          `/club-login.html?next=${encodeURIComponent(pathOnly + search)}`,
        );
        res.end();
      })
      .catch(() => {
        res.statusCode = 302;
        res.setHeader("Location", `/club-login.html?next=${encodeURIComponent(pathOnly + search)}`);
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
        app: path.resolve(root, "app.html"),
        clubPilot: path.resolve(root, "club-pilot.html"),
        clubFlight: path.resolve(root, "club-flight.html"),
        clubLogin: path.resolve(root, "club-login.html"),
        clubMap: path.resolve(root, "club-map.html"),
        publicMap: path.resolve(root, "public-map.html"),
        clubRegister: path.resolve(root, "club-register.html"),
        clubSignup: path.resolve(root, "club-signup.html"),
        clubSuspended: path.resolve(root, "club-suspended.html"),
        clubAdminTypes: path.resolve(root, "club-admin-types.html"),
        clubAdminTails: path.resolve(root, "club-admin-tails.html"),
        clubAdminPilots: path.resolve(root, "club-admin-pilots.html"),
        clubAdminCheckouts: path.resolve(root, "club-admin-checkouts.html"),
        clubAdminMatrix: path.resolve(root, "club-admin-matrix.html"),
        clubAdminImport: path.resolve(root, "club-admin-import.html"),
        clubAdminAudit: path.resolve(root, "club-admin-audit.html"),
        clubAdminMap: path.resolve(root, "club-admin-map.html"),
        clubAdminKioskLayout: path.resolve(root, "club-admin-kiosk-layout.html"),
        clubScreen: path.resolve(root, "club-screen.html"),
        clubCheckinLanding: path.resolve(root, "club-checkin-landing.html"),
        clubCheckinExpired: path.resolve(root, "club-checkin-expired.html"),
        shareFlight: path.resolve(root, "share-flight.html"),
        e2eLive: path.resolve(root, "e2e-live.html"),
      },
    },
  },
});
