import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initI18n, setLocale } from "../i18n/index";
import {
  isAdminNavLinkActive,
  mountAdminNav,
  normalizeAdminPathname,
  performAccountLogout,
  resetAdminNavStateForTests,
  resolveActiveClubLabel,
} from "./adminCommon";

describe("adminCommon header and navigation helpers", () => {
  beforeEach(() => {
    initI18n({ stored: "es" });
    resetAdminNavStateForTests();
    window.history.replaceState({}, "", "/club-admin-types.html");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
    window.history.replaceState({}, "", "/");
  });

  it("normalizes pathnames and matches active admin nav links", () => {
    expect(normalizeAdminPathname("club-admin-types.html")).toBe("/club-admin-types.html");
    expect(normalizeAdminPathname("/club-admin-types.html/")).toBe("/club-admin-types.html");
    expect(normalizeAdminPathname("/club-admin-map.html?tab=1#top")).toBe("/club-admin-map.html");
    expect(normalizeAdminPathname("/")).toBe("/");

    expect(isAdminNavLinkActive("/club-admin-types.html", "/club-admin-types.html")).toBe(true);
    expect(isAdminNavLinkActive("club-admin-types.html", "/club-admin-types.html/")).toBe(true);
    expect(isAdminNavLinkActive("/club-admin-types.html", "/club-admin-tails.html")).toBe(false);
  });

  it("resolves active club label preferring clubName and falling back to clubSlug", () => {
    expect(resolveActiveClubLabel({ clubName: "Club Aéreo de Santiago", clubSlug: "cas" })).toBe(
      "Club Aéreo de Santiago",
    );
    expect(resolveActiveClubLabel({ clubName: "   ", clubSlug: "cas-tobalaba" })).toBe(
      "cas-tobalaba",
    );
    expect(resolveActiveClubLabel({ clubSlug: "demo" })).toBe("demo");
    expect(resolveActiveClubLabel({ clubName: null, clubSlug: null })).toBeNull();
    expect(resolveActiveClubLabel(null)).toBeNull();
  });

  it("renders shared header bar, active aria-current pill, no login link, and caches /api/club/me across locale changes", async () => {
    document.body.innerHTML = `<main class="admin-shell"><div id="admin-nav"></div></main>`;
    window.history.replaceState({}, "", "/club-admin-pilots.html");

    let clubMeCalls = 0;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/club/me") {
        clubMeCalls += 1;
        return new Response(
          JSON.stringify({
            pilot: { name: "Demo Admin", role: "admin" },
            clubSlug: "demo",
            clubName: "Club Aéreo <Demo>",
          }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/pending-counts") {
        return new Response(JSON.stringify({ pilots: 2, accessRequests: 1 }), { status: 200 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    mountAdminNav();

    const headerBar = document.querySelector<HTMLDivElement>(".admin-header-bar")!;
    const nav = document.querySelector<HTMLElement>("nav.admin-nav")!;
    expect(headerBar).not.toBeNull();
    expect(nav).not.toBeNull();
    // Header bar is rendered above nav inside #admin-nav.
    expect(headerBar.nextElementSibling).toBe(nav);

    // Header links and logout button.
    const myClubs = headerBar.querySelector<HTMLAnchorElement>('a[href="/account-clubs.html"]')!;
    const myAccount = headerBar.querySelector<HTMLAnchorElement>('a[href="/account.html"]')!;
    const logoutBtn = headerBar.querySelector<HTMLButtonElement>("#admin-header-logout")!;
    expect(myClubs.textContent).toBe("Mis clubes");
    expect(myAccount.textContent).toBe("Mi cuenta");
    expect(logoutBtn.textContent).toBe("Cerrar sesión");

    // No "Iniciar sesión" / login?mode=club in admin nav.
    const navLinks = [...nav.querySelectorAll<HTMLAnchorElement>("a")];
    expect(navLinks).toHaveLength(8);
    expect(navLinks.some((a) => a.getAttribute("href")?.includes("login"))).toBe(false);
    expect(nav.textContent).not.toContain("Iniciar sesión");

    // Only the matching pathname link gets aria-current="page".
    const activeLinks = nav.querySelectorAll<HTMLAnchorElement>('a[aria-current="page"]');
    expect(activeLinks).toHaveLength(1);
    expect(activeLinks[0].getAttribute("href")).toBe("/club-admin-pilots.html");

    // Active club name populated safely via textContent (no HTML injection).
    const clubEl = document.querySelector<HTMLSpanElement>("#admin-header-club")!;
    await vi.waitFor(() => {
      expect(clubEl.hidden).toBe(false);
      expect(clubEl.textContent).toBe("Club: Club Aéreo <Demo>");
    });
    expect(clubEl.children).toHaveLength(0);
    expect(clubMeCalls).toBe(1);

    // Locale switch re-renders labels using cached club info without re-fetching /api/club/me.
    await setLocale("en", { persistUser: false });
    expect(clubMeCalls).toBe(1);
    expect(document.querySelector("#admin-header-club")?.textContent).toBe(
      "Club: Club Aéreo <Demo>",
    );
    expect(
      document.querySelector('.admin-header-bar a[href="/account-clubs.html"]')?.textContent,
    ).toBe("My clubs");
    expect(document.querySelector('.admin-header-bar a[href="/account.html"]')?.textContent).toBe(
      "My account",
    );
    expect(document.querySelector("#admin-header-logout")?.textContent).toBe("Log out");
    expect(
      document.querySelector('nav.admin-nav a[aria-current="page"]')?.getAttribute("href"),
    ).toBe("/club-admin-pilots.html");

    // Calling mountAdminNav again does not duplicate the /api/club/me request.
    mountAdminNav();
    expect(clubMeCalls).toBe(1);
  });

  it("falls back to clubSlug when clubName is missing", async () => {
    document.body.innerHTML = `<main class="admin-shell"><div id="admin-nav"></div></main>`;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        if (String(input) === "/api/club/me") {
          return new Response(
            JSON.stringify({ pilot: { name: "Admin", role: "admin" }, clubSlug: "cas-tobalaba" }),
            { status: 200 },
          );
        }
        return new Response("{}", { status: 404 });
      }),
    );

    mountAdminNav();
    const clubEl = document.querySelector<HTMLSpanElement>("#admin-header-club")!;
    await vi.waitFor(() => {
      expect(clubEl.hidden).toBe(false);
      expect(clubEl.textContent).toBe("Club: cas-tobalaba");
    });
  });

  it("performAccountLogout POSTs /api/account/logout, broadcasts logout signal, and redirects to /login", async () => {
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ ok: true }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    const navSpy = vi.fn();
    localStorage.clear();

    await performAccountLogout(navSpy);

    expect(fetchMock).toHaveBeenCalledWith("/api/account/logout", {
      method: "POST",
      credentials: "same-origin",
    });
    expect(localStorage.getItem("fv-kiosk-sync")).toMatch(/^logout:\d+:/);
    expect(navSpy).toHaveBeenCalledWith("/login");
  });
});
