import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initI18n } from "../i18n/index";
import {
  bootMisClubs,
  findSoleAutoActivateMembershipId,
  isStaffRole,
  membershipRoleLabel,
  membershipStatusLabel,
  resolveAdminNext,
  shouldShowContinue,
} from "./misClubs";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

function loadHtmlBody(fileName: string): string {
  const html = fs.readFileSync(path.join(root, fileName), "utf8");
  const match = html.match(/<body[^>]*>([\s\S]*?)<\/body>/i);
  return match ? match[1] : html;
}

describe("misClubs pure helpers", () => {
  beforeEach(() => {
    initI18n({ stored: "es" });
  });

  it("shows Continuar only for explicit non-default safe ?next=", () => {
    expect(shouldShowContinue("")).toBe(false);
    expect(shouldShowContinue("?next=")).toBe(false);
    expect(shouldShowContinue("?next=/")).toBe(false);
    expect(shouldShowContinue("?next=%2F")).toBe(false);
    expect(shouldShowContinue("?next=https://evil.test/foo")).toBe(false);
    expect(shouldShowContinue("?next=/login")).toBe(false);
    expect(shouldShowContinue("?next=/club-pilot.html")).toBe(true);
    expect(shouldShowContinue("?next=%2Fclub-admin-map.html")).toBe(true);
    expect(shouldShowContinue("?next=/vuelo-5")).toBe(true);
  });

  it("resolves adminNext safely defaulting to /club-admin-map.html", () => {
    expect(resolveAdminNext("")).toBe("/club-admin-map.html");
    expect(resolveAdminNext("?adminNext=/club-admin-tails.html")).toBe("/club-admin-tails.html");
    expect(resolveAdminNext("?adminNext=https://evil.test")).toBe("/");
  });

  it("identifies sole active membership to auto-activate when none is active yet", () => {
    expect(findSoleAutoActivateMembershipId(null, [])).toBeNull();
    expect(
      findSoleAutoActivateMembershipId(null, [{ id: "m1", status: "pending", role: "pilot" }]),
    ).toBeNull();
    expect(
      findSoleAutoActivateMembershipId(null, [
        { id: "m1", status: "active", role: "admin" },
        { id: "m2", status: "pending", role: "pilot" },
      ]),
    ).toBe("m1");
    expect(
      findSoleAutoActivateMembershipId("m1", [{ id: "m1", status: "active", role: "admin" }]),
    ).toBeNull();
    expect(
      findSoleAutoActivateMembershipId(null, [
        { id: "m1", status: "active", role: "admin" },
        { id: "m2", status: "active", role: "pilot" },
      ]),
    ).toBeNull();
  });

  it("formats role and status labels in es-CL and en", () => {
    expect(isStaffRole("admin")).toBe(true);
    expect(isStaffRole("staff")).toBe(true);
    expect(isStaffRole("pilot")).toBe(false);
    expect(membershipRoleLabel("admin")).toBe("Administrador");
    expect(membershipRoleLabel("staff")).toBe("Encargado");
    expect(membershipRoleLabel("pilot")).toBe("Piloto");
    expect(membershipStatusLabel("active")).toBe("activo");
    expect(membershipStatusLabel("pending")).toBe("pendiente");

    initI18n({ stored: "en" });
    expect(membershipRoleLabel("admin")).toBe("Administrator");
    expect(membershipRoleLabel("pilot")).toBe("Pilot");
    expect(membershipStatusLabel("active")).toBe("active");
  });
});

describe("misClubs DOM flow", () => {
  beforeEach(() => {
    initI18n({ stored: "es" });
    localStorage.setItem("fv_locale", "es");
    document.body.innerHTML = loadHtmlBody("account-clubs.html");
    window.history.replaceState({}, "", "/account-clubs.html");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
    localStorage.removeItem("fv_locale");
    window.history.replaceState({}, "", "/");
  });

  it("removes legacy manual slug form, auto-activates sole active admin club, shows email/badge/links, and navigates via Administrar", async () => {
    expect(document.querySelector("#admin-panel")).toBeNull();
    expect(document.querySelector("#admin-open")).toBeNull();
    expect(document.querySelector('a[href="/account.html"]')).not.toBeNull();
    expect(document.querySelector('a[href="/"]')).not.toBeNull();
    expect(document.querySelector("#logout-btn")).not.toBeNull();

    let activeId: string | null = null;
    const switchCalls: string[] = [];
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/club/i18n/context") {
        return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
      }
      if (url === "/api/account/me") {
        return new Response(
          JSON.stringify({
            account: { id: "a1", email: "admin@demo.test" },
            activeMembershipId: activeId,
            memberships: [
              { id: "m-admin", slug: "demo", name: "Club Demo", role: "admin", status: "active" },
            ],
          }),
          { status: 200 },
        );
      }
      if (url === "/api/account/switch-club") {
        const body = JSON.parse(String(init?.body ?? "{}")) as { membershipId: string };
        switchCalls.push(body.membershipId);
        activeId = body.membershipId;
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      if (url === "/api/account/logout") {
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const navigated: string[] = [];
    const replaced: string[] = [];
    await bootMisClubs({
      navigate: (u) => navigated.push(u),
      replace: (u) => replaced.push(u),
    });

    // Sole active membership was auto-activated on load.
    expect(switchCalls).toEqual(["m-admin"]);
    expect(document.querySelector("#account-email")?.textContent).toBe("admin@demo.test");
    expect((document.querySelector("#account-email") as HTMLElement).hidden).toBe(false);
    expect((document.querySelector("#continue-globe") as HTMLButtonElement).hidden).toBe(true);

    const card = document.querySelector<HTMLLIElement>('li.club-card[data-membership-id="m-admin"]')!;
    expect(card).not.toBeNull();
    expect(card.classList.contains("is-current")).toBe(true);
    expect(card.querySelector(".club-card-name")?.textContent).toBe("Club Demo");
    expect(card.querySelector(".club-current-badge")?.textContent).toBe("Club actual");
    expect(card.querySelector(".club-card-meta")?.textContent).toBe("Administrador · activo");
    expect(card.textContent).not.toContain("(activo)");

    const adminBtn = card.querySelector<HTMLButtonElement>(".club-card-admin")!;
    expect(adminBtn).not.toBeNull();
    expect(adminBtn.classList.contains("btn-primary")).toBe(true);
    expect(adminBtn.textContent).toBe("Administrar");

    adminBtn.click();
    await vi.waitFor(() => {
      expect(navigated).toEqual(["/club-admin-map.html"]);
    });
    expect(switchCalls).toEqual(["m-admin", "m-admin"]);

    // Logout button calls POST /api/account/logout and replaces to /login.
    document.querySelector<HTMLButtonElement>("#logout-btn")!.click();
    await vi.waitFor(() => {
      expect(replaced).toEqual(["/login"]);
    });
  });

  it("shows Continuar only with explicit non-default ?next=, skips auto-activate with multiple active clubs, and supports pilot selection + auto=1", async () => {
    window.history.replaceState(
      {},
      "",
      "/account-clubs.html?next=%2Fclub-pilot.html&auto=1&adminNext=%2Fclub-admin-tails.html",
    );

    let activeId: string | null = null;
    const switchCalls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url === "/api/club/i18n/context") {
          return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
        }
        if (url === "/api/account/me") {
          return new Response(
            JSON.stringify({
              account: { id: "a2", email: "multi@demo.test" },
              activeMembershipId: activeId,
              memberships: [
                { id: "m-pilot", slug: "alpha", name: "Club Alpha", role: "pilot", status: "active" },
                { id: "m-admin", slug: "beta", name: "Club Beta", role: "admin", status: "active" },
                { id: "m-pend", slug: "gamma", name: "Club Gamma", role: "pilot", status: "pending" },
              ],
            }),
            { status: 200 },
          );
        }
        if (url === "/api/account/switch-club") {
          const body = JSON.parse(String(init?.body ?? "{}")) as { membershipId: string };
          switchCalls.push(body.membershipId);
          activeId = body.membershipId;
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const navigated: string[] = [];
    await bootMisClubs({ navigate: (u) => navigated.push(u), replace: () => {} });

    // Two active memberships: must NOT auto-activate.
    expect(switchCalls).toEqual([]);
    expect((document.querySelector("#continue-globe") as HTMLButtonElement).hidden).toBe(false);
    expect(document.querySelectorAll(".club-current-badge")).toHaveLength(0);

    // Pending card has no action buttons.
    const pendCard = document.querySelector('li[data-membership-id="m-pend"]')!;
    expect(pendCard.querySelector(".club-card-meta")?.textContent).toBe("Piloto · pendiente");
    expect(pendCard.querySelectorAll("button")).toHaveLength(0);

    // Clicking Usar este club on pilot switches, re-renders badge, and follows next because auto=1.
    const pilotSelect = document.querySelector<HTMLButtonElement>(
      'li[data-membership-id="m-pilot"] .club-card-select',
    )!;
    expect(pilotSelect.textContent).toBe("Usar este club");
    pilotSelect.click();
    await vi.waitFor(() => {
      expect(navigated).toEqual(["/club-pilot.html"]);
    });
    expect(switchCalls).toEqual(["m-pilot"]);
    expect(
      document.querySelector('li[data-membership-id="m-pilot"] .club-current-badge')?.textContent,
    ).toBe("Club actual");

    // Clicking Administrar on Club Beta switches to m-admin and honours adminNext.
    document
      .querySelector<HTMLButtonElement>('li[data-membership-id="m-admin"] .club-card-admin')!
      .click();
    await vi.waitFor(() => {
      expect(navigated).toEqual(["/club-pilot.html", "/club-admin-tails.html"]);
    });
    expect(switchCalls).toEqual(["m-pilot", "m-admin"]);
  });
});
