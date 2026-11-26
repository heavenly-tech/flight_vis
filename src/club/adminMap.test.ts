import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

describe("admin map page without display activation token card", () => {
  it("keeps the Abrir pantalla del club slot and removes display activation controls", () => {
    const html = fs.readFileSync(path.join(root, "club-admin-map.html"), "utf8");
    const container = document.createElement("div");
    container.innerHTML = html;
    expect(container.querySelector("#open-kiosk-slot")).not.toBeNull();
    expect(container.querySelector("#kiosk-card")).toBeNull();
    expect(container.querySelector("#btn-kiosk")).toBeNull();
    expect(container.querySelector("#kiosk-url")).toBeNull();
    expect(container.querySelector("#kiosk-copy-status")).toBeNull();
  });
});

describe("kiosk layout decorative QR placeholder", () => {
  it("uses an inline decorative aria-hidden SVG and never references /api/club/screen/qr.svg", () => {
    const html = fs.readFileSync(path.join(root, "club-admin-kiosk-layout.html"), "utf8");
    expect(html).not.toContain("/api/club/screen/qr.svg");
    const container = document.createElement("div");
    container.innerHTML = html;
    const previewQr = container.querySelector("#preview-qr");
    expect(previewQr).not.toBeNull();
    expect(previewQr?.querySelector("img")).toBeNull();
    const svg = previewQr?.querySelector("svg.qr-placeholder");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
    expect(previewQr?.querySelector("#resize-handle")).not.toBeNull();
  });
});
