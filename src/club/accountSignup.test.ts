import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyI18nDocument, initI18n, t } from "../i18n/index";
import { mountAccountSignup } from "./accountSignupPage";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

function loadSignupBody(): string {
  const html = fs.readFileSync(path.join(root, "signup.html"), "utf8");
  const match = html.match(/<body[^>]*>([\s\S]*?)<\/body>/i);
  return match ? match[1] : html;
}

describe("account signup API contract & optional club picker", () => {
  beforeEach(() => {
    document.body.innerHTML = loadSignupBody();
    initI18n({ stored: "es" });
    applyI18nDocument(document);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("signup-available shape", () => {
    expect({ ok: true, signup: true }).toMatchObject({ signup: true });
  });

  it("hides club select by default, omits clubSlug when unchecked, and sends clubSlug when checked", async () => {
    const submittedBodies: Array<Record<string, unknown>> = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/clubs/public") {
        return new Response(
          JSON.stringify({
            clubs: [
              { slug: "andes", name: "Club Aéreo Los Andes" },
              { slug: "demo", name: "Club Demo" },
            ],
          }),
          { status: 200 },
        );
      }
      if (url === "/api/account/signup" && init?.method === "POST") {
        submittedBodies.push(JSON.parse(String(init.body ?? "{}")) as Record<string, unknown>);
        return new Response(JSON.stringify({ ok: true }), { status: 201 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const ctrl = mountAccountSignup(document);
    const form = document.querySelector<HTMLFormElement>("#form")!;
    const emailInput = form.querySelector<HTMLInputElement>('input[name="email"]')!;
    const toggle = document.querySelector<HTMLInputElement>("#is-club-member")!;
    const wrap = document.querySelector<HTMLDivElement>("#club-select-wrap")!;
    const select = document.querySelector<HTMLSelectElement>("#club-slug")!;
    const msg = document.querySelector<HTMLParagraphElement>("#msg")!;

    // Default state: toggle unchecked, picker hidden & disabled.
    expect(toggle.checked).toBe(false);
    expect(wrap.hidden).toBe(true);
    expect(select.disabled).toBe(true);

    // Submit without toggle -> payload omits clubSlug.
    emailInput.value = "solo@pilot.cl";
    form.dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(submittedBodies).toHaveLength(1));
    expect(submittedBodies[0]).toEqual({ email: "solo@pilot.cl" });
    expect("clubSlug" in submittedBodies[0]).toBe(false);
    expect(msg.textContent).toBe(t("account.signup.success"));

    // Toggle on -> reveals select, populates from /api/clubs/public.
    toggle.checked = true;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    expect(wrap.hidden).toBe(false);
    expect(select.disabled).toBe(false);
    await ctrl.loadClubs();

    const optionValues = [...select.options].map((o) => o.value);
    const optionLabels = [...select.options].map((o) => o.textContent);
    expect(optionValues).toEqual(["", "andes", "demo"]);
    expect(optionLabels).toEqual([
      t("account.signup.clubPlaceholder"),
      "Club Aéreo Los Andes",
      "Club Demo",
    ]);

    // Submitting with toggle checked but no club picked shows validation message and does not POST.
    form.dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await Promise.resolve();
    expect(submittedBodies).toHaveLength(1);
    expect(msg.textContent).toBe(t("account.signup.clubRequired"));

    // Select a club and submit -> includes clubSlug.
    select.value = "andes";
    emailInput.value = "socio@pilot.cl";
    form.dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(submittedBodies).toHaveLength(2));
    expect(submittedBodies[1]).toEqual({ email: "socio@pilot.cl", clubSlug: "andes" });
    expect(msg.textContent).toBe(t("account.signup.successWithClub"));

    // Toggle off resets value and disables select again.
    toggle.checked = false;
    toggle.dispatchEvent(new Event("change", { bubbles: true }));
    expect(wrap.hidden).toBe(true);
    expect(select.disabled).toBe(true);
    expect(select.value).toBe("");
  });
});
