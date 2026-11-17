import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type ProxyOptions } from "vitest/config";
import { viteStaticCopy } from "vite-plugin-static-copy";

const root = path.dirname(fileURLToPath(import.meta.url));
const cesiumSource = "node_modules/cesium/Build/Cesium";
const cesiumBaseUrl = "cesiumStatic";
const api = "http://127.0.0.1:8080";

const authProxy: Record<string, string | ProxyOptions> = {
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

export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "jsdom",
  },
  base: "/",
  define: {
    CESIUM_BASE_URL: JSON.stringify(`/${cesiumBaseUrl}/`),
  },
  plugins: [
    viteStaticCopy({
      targets: [
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
        clubAdminTypes: path.resolve(root, "club-admin-types.html"),
        clubAdminTails: path.resolve(root, "club-admin-tails.html"),
        clubAdminPilots: path.resolve(root, "club-admin-pilots.html"),
        clubAdminCheckouts: path.resolve(root, "club-admin-checkouts.html"),
        clubAdminMap: path.resolve(root, "club-admin-map.html"),
        clubAdminKioskLayout: path.resolve(root, "club-admin-kiosk-layout.html"),
        clubScreen: path.resolve(root, "club-screen.html"),
        clubCheckinLanding: path.resolve(root, "club-checkin-landing.html"),
        clubCheckinExpired: path.resolve(root, "club-checkin-expired.html"),
        shareFlight: path.resolve(root, "share-flight.html"),
      },
    },
  },
});
