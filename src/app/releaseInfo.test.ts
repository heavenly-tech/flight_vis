// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from "vitest";
import { renderReleaseInfo } from "./releaseInfo";

describe("renderReleaseInfo", () => {
  let versionEl: HTMLElement;
  let noteEl: HTMLElement;

  beforeEach(() => {
    versionEl = document.createElement("p");
    versionEl.id = "version";
    versionEl.hidden = true;
    noteEl = document.createElement("p");
    noteEl.id = "note";
    noteEl.hidden = true;
    document.body.appendChild(versionEl);
    document.body.appendChild(noteEl);
  });

  it("shows version and note when both are present", () => {
    const data = {
      available: true,
      version: "1.1.0+2",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "Desinstala la app anterior antes de instalar esta versión.",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(false);
    expect(versionEl.textContent).toBe("Versión 1.1.0+2");
    expect(noteEl.hidden).toBe(false);
    expect(noteEl.textContent).toBe("Desinstala la app anterior antes de instalar esta versión.");
  });

  it("hides version when version is null", () => {
    const data = {
      available: true,
      version: null,
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "Test message",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(true);
    expect(noteEl.hidden).toBe(false);
    expect(noteEl.textContent).toBe("Test message");
  });

  it("hides version when version is empty string", () => {
    const data = {
      available: true,
      version: "",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "Test message",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(true);
    expect(noteEl.hidden).toBe(false);
  });

  it("hides version when version is whitespace only", () => {
    const data = {
      available: true,
      version: "   ",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "Test message",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(true);
    expect(noteEl.hidden).toBe(false);
  });

  it("hides note when message is absent", () => {
    const data = {
      available: true,
      version: "1.0.0",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(false);
    expect(versionEl.textContent).toBe("Versión 1.0.0");
    expect(noteEl.hidden).toBe(true);
  });

  it("hides note when message is empty string", () => {
    const data = {
      available: true,
      version: "1.0.0",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(false);
    expect(noteEl.hidden).toBe(true);
  });

  it("hides note when message is whitespace only", () => {
    const data = {
      available: true,
      version: "1.0.0",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "   \n  ",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(false);
    expect(noteEl.hidden).toBe(true);
  });

  it("hides both when version and message are absent", () => {
    const data = {
      available: true,
      version: null,
      sha256: null,
      apkUrl: "/app/flightvis.apk",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(versionEl.hidden).toBe(true);
    expect(noteEl.hidden).toBe(true);
  });

  it("renders HTML entities literally (no innerHTML)", () => {
    const data = {
      available: true,
      version: "1.0.0",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "<b>x</b>",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(noteEl.textContent).toBe("<b>x</b>");
    expect(noteEl.querySelector("b")).toBeNull();
  });

  it("trims message whitespace", () => {
    const data = {
      available: true,
      version: "1.0.0",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "  Test message  \n",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(noteEl.textContent).toBe("Test message");
  });
});
