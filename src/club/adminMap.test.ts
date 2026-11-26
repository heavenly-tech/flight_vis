import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { canConfirmClubDelete, mountDeleteClubSection } from "./adminMap";
import { initI18n } from "../i18n/index";

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

describe("club danger zone (delete club)", () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
    document.body.innerHTML = "";
  });

  function mountDangerZoneHtml(): void {
    initI18n({ stored: "es" });
    const html = fs.readFileSync(path.join(root, "club-admin-map.html"), "utf8");
    const doc = new DOMParser().parseFromString(html, "text/html");
    const details = doc.querySelector("#club-settings");
    expect(details).not.toBeNull();
    document.body.appendChild(document.importNode(details!, true));
  }

  it("canConfirmClubDelete enforces exact trimmed case-sensitive match", () => {
    expect(canConfirmClubDelete("demo", "demo")).toBe(true);
    expect(canConfirmClubDelete("  demo  ", "demo")).toBe(true);
    expect(canConfirmClubDelete("DEMO", "demo")).toBe(false);
    expect(canConfirmClubDelete("Demo", "demo")).toBe(false);
    expect(canConfirmClubDelete("demo-2", "demo")).toBe(false);
    expect(canConfirmClubDelete("", "demo")).toBe(false);
    expect(canConfirmClubDelete("demo", null)).toBe(false);
    expect(canConfirmClubDelete("demo", "")).toBe(false);
  });

  it("hides danger zone for staff and reveals it for admin", () => {
    mountDangerZoneHtml();
    const ctrl = mountDeleteClubSection(vi.fn());
    const section = document.querySelector<HTMLElement>("#club-danger-zone")!;
    const btn = document.querySelector<HTMLButtonElement>("#club-delete-confirm")!;

    expect(section.hidden).toBe(true);
    expect(btn.disabled).toBe(true);

    ctrl.configure({ isAdmin: false, clubSlug: "demo" });
    expect(section.hidden).toBe(true);
    expect(btn.disabled).toBe(true);

    ctrl.configure({ isAdmin: true, clubSlug: "demo" });
    expect(section.hidden).toBe(false);
    expect(document.querySelector("#club-delete-slug-expected")?.textContent).toBe("demo");
    expect(btn.disabled).toBe(true);
  });

  it("enables confirm only for exact slug, submits DELETE, and follows redirect", async () => {
    mountDangerZoneHtml();
    const navigate = vi.fn();
    const ctrl = mountDeleteClubSection(navigate);
    ctrl.configure({ isAdmin: true, clubSlug: "mi-club" });

    const input = document.querySelector<HTMLInputElement>("#club-delete-slug-input")!;
    const btn = document.querySelector<HTMLButtonElement>("#club-delete-confirm")!;

    input.value = "MI-CLUB";
    input.dispatchEvent(new Event("input"));
    expect(btn.disabled).toBe(true);

    input.value = "mi-club";
    input.dispatchEvent(new Event("input"));
    expect(btn.disabled).toBe(false);

    // Renaming slug invalidates confirmation until admin types the new slug.
    ctrl.updateSlug("nuevo-club");
    expect(btn.disabled).toBe(true);
    expect(document.querySelector("#club-delete-slug-expected")?.textContent).toBe("nuevo-club");

    input.value = "  nuevo-club ";
    input.dispatchEvent(new Event("input"));
    expect(btn.disabled).toBe(false);

    const fetchMock = vi.fn(async () => ({
      ok: true,
      json: async () => ({ ok: true, redirect: "/account-clubs.html" }),
    }));
    globalThis.fetch = fetchMock as unknown as typeof fetch;

    btn.click();
    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith("/account-clubs.html"));

    expect(fetchMock).toHaveBeenCalledWith("/api/admin/club", {
      method: "DELETE",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ slug: "nuevo-club" }),
    });
  });

  it("shows localized error message on failed DELETE response", async () => {
    mountDangerZoneHtml();
    const navigate = vi.fn();
    const ctrl = mountDeleteClubSection(navigate);
    ctrl.configure({ isAdmin: true, clubSlug: "mi-club" });

    const input = document.querySelector<HTMLInputElement>("#club-delete-slug-input")!;
    const btn = document.querySelector<HTMLButtonElement>("#club-delete-confirm")!;
    const msg = document.querySelector<HTMLParagraphElement>("#club-delete-msg")!;

    input.value = "mi-club";
    input.dispatchEvent(new Event("input"));

    globalThis.fetch = vi.fn(async () => ({
      ok: false,
      status: 400,
      json: async () => ({ error: "slug mismatch" }),
    })) as unknown as typeof fetch;

    btn.click();
    await vi.waitFor(() =>
      expect(msg.textContent).toBe("El nombre corto ingresado no coincide con este club."),
    );
    expect(navigate).not.toHaveBeenCalled();
    expect(btn.disabled).toBe(false);
  });
});
