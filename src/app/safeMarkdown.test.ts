import { describe, expect, it } from "vitest";
import { flattenCatalog } from "../i18n/catalog";
import {
  isSafeHref,
  renderMarkdownInto,
  renderSafeInlineMarkdown,
  renderSafeMarkdown,
} from "./safeMarkdown";

function renderBlockHtml(md: string): HTMLDivElement {
  const div = document.createElement("div");
  div.appendChild(renderSafeMarkdown(md));
  return div;
}

function renderInlineHtml(md: string): HTMLSpanElement {
  const span = document.createElement("span");
  span.appendChild(renderSafeInlineMarkdown(md));
  return span;
}

describe("isSafeHref", () => {
  it("allows https and safe relative URLs", () => {
    expect(isSafeHref("https://flight.heavenly.cl/app.html")).toBe(
      "https://flight.heavenly.cl/app.html",
    );
    expect(isSafeHref("/app.html")).toBe("/app.html");
    expect(isSafeHref("./flightvis.apk")).toBe("./flightvis.apk");
    expect(isSafeHref("../login")).toBe("../login");
    expect(isSafeHref("#sideload-steps")).toBe("#sideload-steps");
  });

  it("rejects javascript, data, http, protocol-relative, and control-char URLs", () => {
    const blocked = [
      "",
      "   ",
      "javascript:alert(1)",
      "JavaScript:alert(1)",
      "java\tscript:alert(1)",
      "java\nscript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
      "vbscript:msgbox(1)",
      "http://example.com/apk",
      "//evil.example/phish",
      "/\\evil.example/phish",
      "mailto:test@example.com",
      "file:///etc/passwd",
      "https://example.com/has space",
    ];
    for (const url of blocked) {
      expect(isSafeHref(url), `expected blocked: ${JSON.stringify(url)}`).toBeNull();
    }
  });
});

describe("renderSafeMarkdown", () => {
  it("renders paragraphs separated by blank lines and bold text", () => {
    const el = renderBlockHtml(
      "Primera línea\nsegunda línea.\n\n**Importante:** desinstala la beta anterior.",
    );
    const ps = el.querySelectorAll("p");
    expect(ps).toHaveLength(2);
    expect(ps[0].textContent).toBe("Primera línea segunda línea.");
    expect(ps[1].querySelector("strong")?.textContent).toBe("Importante:");
    expect(ps[1].textContent).toBe("Importante: desinstala la beta anterior.");
  });

  it("does not misparse a bold line at start of paragraph as a list item", () => {
    const el = renderBlockHtml("**Cambios importantes** en esta versión.");
    expect(el.querySelector("ul")).toBeNull();
    expect(el.querySelectorAll("p")).toHaveLength(1);
    expect(el.querySelector("strong")?.textContent).toBe("Cambios importantes");
  });

  it("renders unordered and ordered lists with inline formatting", () => {
    const el = renderBlockHtml(
      [
        "Novedades:",
        "",
        "- **Paquete nuevo** en Android",
        "* Ver [guía](/app.html)",
        "",
        "1. Abre **Descargas**",
        "2. Instala `flightvis.apk`",
      ].join("\n"),
    );
    expect(el.querySelectorAll("p")).toHaveLength(1);
    const ulItems = el.querySelectorAll("ul > li");
    expect(ulItems).toHaveLength(2);
    expect(ulItems[0].querySelector("strong")?.textContent).toBe("Paquete nuevo");
    const link = ulItems[1].querySelector("a");
    expect(link?.getAttribute("href")).toBe("/app.html");
    expect(link?.textContent).toBe("guía");

    const olItems = el.querySelectorAll("ol > li");
    expect(olItems).toHaveLength(2);
    expect(olItems[0].querySelector("strong")?.textContent).toBe("Descargas");
    expect(olItems[1].querySelector("code")?.textContent).toBe("flightvis.apk");
  });

  it("renders https links with noopener noreferrer and strips unsafe links to plain text", () => {
    const el = renderBlockHtml(
      [
        "Seguro: [sitio](https://heavenly.cl/docs) y [app](/app.html).",
        "",
        "Bloqueados: [xss](javascript:alert(1)) [http](http://example.com) [proto](//evil.example) [data](data:text/html,hi).",
      ].join("\n"),
    );
    const anchors = [...el.querySelectorAll("a")];
    expect(anchors).toHaveLength(2);
    expect(anchors[0].getAttribute("href")).toBe("https://heavenly.cl/docs");
    expect(anchors[0].getAttribute("rel")).toBe("noopener noreferrer");
    expect(anchors[1].getAttribute("href")).toBe("/app.html");
    expect(anchors[1].getAttribute("rel")).toBeNull();
    expect(el.textContent).toContain("[xss](javascript:alert(1))");
    expect(el.textContent).toContain("[http](http://example.com)");
  });

  it("escapes raw HTML tags and XSS payloads via text nodes", () => {
    const payload =
      '<script>alert("xss")</script> <img src=x onerror=alert(1)> **<b>negrita</b>** [<svg onload=alert(1)>](https://heavenly.cl/)';
    const el = renderBlockHtml(payload);
    expect(el.querySelector("script")).toBeNull();
    expect(el.querySelector("img")).toBeNull();
    expect(el.querySelector("b")).toBeNull();
    expect(el.querySelector("svg")).toBeNull();
    expect(el.textContent).toContain('<script>alert("xss")</script>');
    expect(el.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(el.querySelector("strong")?.textContent).toBe("<b>negrita</b>");
    expect(el.querySelector("a")?.textContent).toBe("<svg onload=alert(1)>");
  });

  it("keeps unmatched or empty markdown delimiters as plain text", () => {
    const unmatched = renderInlineHtml("Texto con **sin cerrar y [roto](sin-cerrar");
    expect(unmatched.querySelector("strong")).toBeNull();
    expect(unmatched.querySelector("a")).toBeNull();
    expect(unmatched.textContent).toBe("Texto con **sin cerrar y [roto](sin-cerrar");

    const emptyBold = renderInlineHtml("Texto con **** vacío");
    expect(emptyBold.querySelector("strong")).toBeNull();
    expect(emptyBold.textContent).toBe("Texto con **** vacío");
  });

  it("renders multi-block markdown safely when target is a <p> element", () => {
    const host = document.createElement("section");
    const p = document.createElement("p");
    p.id = "status";
    host.appendChild(p);

    renderMarkdownInto(p, "Solo una línea con **negrita**.");
    expect(p.hidden).toBe(false);
    expect(p.querySelector("strong")?.textContent).toBe("negrita");
    expect(host.querySelector(".release-markdown")).toBeNull();

    renderMarkdownInto(p, "Primer párrafo.\n\n- Punto uno\n- Punto dos");
    expect(p.hidden).toBe(true);
    const box = host.querySelector(".release-markdown");
    expect(box).not.toBeNull();
    expect(box?.querySelectorAll("p")).toHaveLength(1);
    expect(box?.querySelectorAll("ul > li")).toHaveLength(2);
    expect(p.querySelector("p")).toBeNull();
  });
});

describe("/app.html locale copy contracts", () => {
  it("drops step 5, removes forbidden phrases, and renders bold markdown cleanly in es and en", () => {
    for (const locale of ["es", "en"] as const) {
      const cat = flattenCatalog(locale);
      expect(cat["app.sideloadStep5"]).toBeUndefined();
      expect(cat["app.uninstallNoticeTitle"]).toBeTruthy();
      expect(cat["app.uninstallNotice"]).toBeTruthy();

      const steps = [1, 2, 3, 4].map((n) => cat[`app.sideloadStep${n}`]);
      for (const step of steps) {
        expect(step).toBeTruthy();
        expect(step.toLowerCase()).not.toContain("cuando esté disponible");
        expect(step.toLowerCase()).not.toContain("when available");
        expect(step.toLowerCase()).not.toMatch(/\bfixes\b/);
        const el = renderInlineHtml(step);
        expect(el.textContent).not.toContain("**");
        expect(el.querySelectorAll("strong").length).toBeGreaterThan(0);
      }

      const noticeEl = renderInlineHtml(cat["app.uninstallNotice"]);
      expect(noticeEl.textContent).not.toContain("**");
      expect(noticeEl.querySelectorAll("strong").length).toBeGreaterThan(0);
    }
  });
});
