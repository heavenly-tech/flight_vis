import { apiErrorMessage, t } from "../i18n/index";

export interface PublicClubOption {
  slug: string;
  name: string;
}

export interface AccountSignupController {
  loadClubs: () => Promise<PublicClubOption[]>;
  syncClubPicker: () => void;
}

declare global {
  interface Window {
    __fvAccountSignupReady?: boolean;
  }
}

export function mountAccountSignup(root: ParentNode = document): AccountSignupController {
  const form = root.querySelector<HTMLFormElement>("#form");
  const msg = root.querySelector<HTMLParagraphElement>("#msg");
  const clubToggle = root.querySelector<HTMLInputElement>("#is-club-member");
  const clubWrap = root.querySelector<HTMLDivElement>("#club-select-wrap");
  const clubSelect = root.querySelector<HTMLSelectElement>("#club-slug");

  let clubsLoaded = false;
  let cachedClubs: PublicClubOption[] = [];
  let loadingPromise: Promise<PublicClubOption[]> | null = null;

  function renderClubOptions(clubs: PublicClubOption[]): void {
    if (!clubSelect) return;
    const prev = clubSelect.value;
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.dataset.i18n = clubs.length > 0 ? "account.signup.clubPlaceholder" : "account.signup.noClubsAvailable";
    placeholder.textContent = t(
      clubs.length > 0 ? "account.signup.clubPlaceholder" : "account.signup.noClubsAvailable",
    );
    const options = clubs.map((club) => {
      const opt = document.createElement("option");
      opt.value = club.slug;
      opt.textContent = club.name;
      return opt;
    });
    clubSelect.replaceChildren(placeholder, ...options);
    if (prev && clubs.some((c) => c.slug === prev)) {
      clubSelect.value = prev;
    } else {
      clubSelect.value = "";
    }
  }

  async function loadClubs(): Promise<PublicClubOption[]> {
    if (clubsLoaded) return cachedClubs;
    if (loadingPromise) return loadingPromise;
    loadingPromise = (async () => {
      try {
        const res = await fetch("/api/clubs/public", {
          headers: { Accept: "application/json" },
          credentials: "same-origin",
        });
        if (!res.ok) return [];
        const data = (await res.json().catch(() => ({}))) as { clubs?: unknown };
        const raw = Array.isArray(data.clubs) ? data.clubs : [];
        const list: PublicClubOption[] = [];
        for (const item of raw) {
          if (
            item &&
            typeof item === "object" &&
            typeof (item as { slug?: unknown }).slug === "string" &&
            typeof (item as { name?: unknown }).name === "string"
          ) {
            const slug = (item as { slug: string }).slug.trim();
            const name = (item as { name: string }).name.trim();
            if (slug && name) list.push({ slug, name });
          }
        }
        cachedClubs = list;
        clubsLoaded = true;
        renderClubOptions(cachedClubs);
        return cachedClubs;
      } catch {
        return [];
      } finally {
        loadingPromise = null;
      }
    })();
    return loadingPromise;
  }

  function syncClubPicker(): void {
    const enabled = Boolean(clubToggle?.checked);
    if (clubWrap) clubWrap.hidden = !enabled;
    if (clubSelect) {
      clubSelect.disabled = !enabled;
      clubSelect.required = enabled;
      if (!enabled) {
        clubSelect.value = "";
      }
    }
    if (enabled && !clubsLoaded) {
      void loadClubs();
    }
  }

  form?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });

  clubToggle?.addEventListener("change", () => {
    syncClubPicker();
  });

  if (typeof window !== "undefined") {
    window.addEventListener("fv-locale-change", () => {
      if (clubsLoaded) renderClubOptions(cachedClubs);
    });
    window.addEventListener("fv-i18n-ready", () => {
      if (clubsLoaded) renderClubOptions(cachedClubs);
    });
  }

  syncClubPicker();

  form?.addEventListener("submit", async () => {
    if (!form) return;
    const body = Object.fromEntries(new FormData(form));
    const payload: Record<string, string> = { email: String(body.email ?? "") };
    const pw = String(body.password ?? "");
    if (pw) payload.password = pw;
    let chosenClub = "";
    if (clubToggle?.checked) {
      chosenClub = (clubSelect?.value ?? "").trim();
      if (!chosenClub) {
        if (msg) msg.textContent = t("account.signup.clubRequired");
        return;
      }
      payload.clubSlug = chosenClub;
    }
    const res = await fetch("/api/account/signup", {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(payload),
    });
    if (res.ok) {
      if (msg) {
        msg.textContent = chosenClub
          ? t("account.signup.successWithClub")
          : t("account.signup.success");
      }
      if (pw) location.href = "/login";
    } else {
      const err = (await res.json().catch(() => ({}))).error ?? "signup failed";
      if (msg) msg.textContent = apiErrorMessage(String(err));
    }
  });

  return { loadClubs, syncClubPicker };
}

if (typeof document !== "undefined" && document.querySelector('[data-page="account-signup"]')) {
  window.__fvAccountSignupReady = true;
  const controller = mountAccountSignup(document);
  void (async () => {
    const { startClubPage } = await import("./i18nPage");
    await startClubPage({ persistUser: false });
    await controller.loadClubs();
  })();
}
