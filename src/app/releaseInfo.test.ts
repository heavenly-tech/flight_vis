// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from "vitest";
import { renderReleaseInfo } from "./releaseInfo";

describe("renderReleaseInfo", () => {
  let versionEl: HTMLElement;
  let noteEl: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = "";
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

  it("renders safe inline markdown via DOM APIs while keeping raw HTML literal", () => {
    const data = {
      available: true,
      version: "1.2.0",
      sha256: null,
      apkUrl: "/app/flightvis.apk",
      message: "**Importante:** lee la [guía](/app.html), ignora [xss](javascript:alert(1)) y <b>literal</b>.",
    };
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      data,
      (v: string) => `Versión ${v}`,
    );
    expect(noteEl.hidden).toBe(false);
    expect(noteEl.querySelector("strong")?.textContent).toBe("Importante:");
    expect(noteEl.querySelector("a")?.getAttribute("href")).toBe("/app.html");
    expect(noteEl.querySelectorAll("a")).toHaveLength(1);
    expect(noteEl.querySelector("b")).toBeNull();
    expect(noteEl.textContent).toContain("[xss](javascript:alert(1))");
    expect(noteEl.textContent).toContain("<b>literal</b>");
    expect(noteEl.textContent).not.toContain("**");
  });

  it("renders multi-block markdown into a sibling container and cleans it up when cleared", () => {
    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      {
        available: true,
        version: "1.2.0",
        sha256: null,
        apkUrl: "/app/flightvis.apk",
        message: "**Cambios**\n\n- Uno\n- Dos",
      },
      (v: string) => `Versión ${v}`,
    );
    expect(noteEl.hidden).toBe(true);
    const box = document.body.querySelector<HTMLDivElement>('[data-md-for="note"]');
    expect(box).not.toBeNull();
    expect(box?.querySelector("strong")?.textContent).toBe("Cambios");
    expect(box?.querySelectorAll("ul > li")).toHaveLength(2);

    renderReleaseInfo(
      { version: versionEl, note: noteEl },
      {
        available: true,
        version: "1.2.0",
        sha256: null,
        apkUrl: "/app/flightvis.apk",
      },
      (v: string) => `Versión ${v}`,
    );
    expect(noteEl.hidden).toBe(true);
    expect(document.body.querySelector('[data-md-for="note"]')).toBeNull();
  });
});
