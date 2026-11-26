import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { copyTextWithFallback } from "./adminMap";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

describe("copyTextWithFallback", () => {
  const originalClipboard = navigator.clipboard;
  const originalExecCommand = document.execCommand;

  afterEach(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: originalClipboard,
      configurable: true,
    });
    document.execCommand = originalExecCommand;
    document.body.innerHTML = "";
  });

  it("uses navigator.clipboard.writeText when available", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const input = document.createElement("input");
    input.value = "abc-123";
    document.body.appendChild(input);

    const res = await copyTextWithFallback("abc-123", input);
    expect(res).toBe("copied");
    expect(writeText).toHaveBeenCalledWith("abc-123");
  });

  it("falls back to selecting the input and execCommand('copy') when clipboard rejects", async () => {
    const writeText = vi.fn(async () => {
      throw new Error("denied");
    });
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const execSpy = vi.fn(() => true);
    document.execCommand = execSpy;

    const input = document.createElement("input");
    input.value = "tok-xyz";
    const selectSpy = vi.spyOn(input, "select");
    document.body.appendChild(input);

    const res = await copyTextWithFallback("tok-xyz", input);
    expect(res).toBe("copied");
    expect(selectSpy).toHaveBeenCalled();
    expect(execSpy).toHaveBeenCalledWith("copy");
  });

  it("returns 'selected' when clipboard and execCommand both fail", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: undefined,
      configurable: true,
    });
    document.execCommand = vi.fn(() => false);

    const input = document.createElement("input");
    input.value = "tok-fallback";
    const selectSpy = vi.spyOn(input, "select");
    document.body.appendChild(input);

    const res = await copyTextWithFallback("tok-fallback", input);
    expect(res).toBe("selected");
    expect(selectSpy).toHaveBeenCalled();
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
