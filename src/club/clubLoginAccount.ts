import { startClubPage } from "./i18nPage";
import { apiErrorMessage, t } from "../i18n/index";
import { safeNextPath } from "./safeNext";

function staffRoles(role: string): boolean {
  return role === "admin" || role === "staff";
}

declare global {
  interface Window {
    __fvClubLoginReady?: boolean;
  }
}

export async function bootClubLoginAccount(): Promise<void> {
  const params = new URLSearchParams(location.search);
  const mode = params.get("mode") === "club" ? "club" : "pilot";
  const step = params.get("step") ?? "login";
  let clubPrefill = (params.get("club") ?? "").trim().toLowerCase();
  const nextAfterLogin = safeNextPath(
    params.get("next") ?? (mode === "club" ? "/club-admin-map.html" : "/club-pilot.html"),
  );

  const form = document.getElementById("form") as HTMLFormElement;
  const msg = document.getElementById("msg") as HTMLParagraphElement;
  const legacy = document.getElementById("legacy-login") as HTMLFormElement | null;
  const joinForm = document.getElementById("join-form") as HTMLFormElement | null;

  let activeClubSlug = clubPrefill;

  form?.addEventListener(
    "submit",
    (e) => e.preventDefault(),
    { capture: true },
  );
  legacy?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });
  joinForm?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });
  window.__fvClubLoginReady = true;
  const nextField = document.getElementById("next") as HTMLInputElement | null;
  if (nextField) nextField.value = nextAfterLogin;
  const legacyNext = document.getElementById("legacy-next") as HTMLInputElement | null;
  if (legacyNext) legacyNext.value = nextAfterLogin;

  await startClubPage({ persistUser: false });

  const slugPanel = document.getElementById("slug-panel") as HTMLElement;
  const loginPanel = document.getElementById("login-panel") as HTMLElement;
  const pickPanel = document.getElementById("pick-panel") as HTMLElement;
  const joinPanel = document.getElementById("join-panel") as HTMLElement;
  const loginHeading = document.getElementById("login-heading") as HTMLElement;
  const clubSlugLabel = document.getElementById("club-slug-label") as HTMLElement;
  const pilotSignup = document.getElementById("pilot-signup") as HTMLElement;
  const back = document.getElementById("back-chooser") as HTMLAnchorElement;
  const passwordInput = loginPanel.querySelector('input[name="password"]') as HTMLInputElement;

  const nextQs = new URLSearchParams({ next: nextAfterLogin });
  back.href = `/login?${nextQs}`;

  const show = (panel: "slug" | "login" | "pick" | "join") => {
    slugPanel.hidden = panel !== "slug";
    loginPanel.hidden = panel !== "login";
    pickPanel.hidden = panel !== "pick";
    joinPanel.hidden = panel !== "join";
  };

  if (mode === "club") {
    loginHeading.dataset.i18n = "club.login.clubTitle";
    loginHeading.textContent = t("club.login.clubTitle");
    pilotSignup.hidden = true;
    passwordInput.required = false;
    if (!activeClubSlug) {
      show("slug");
    } else {
      clubSlugLabel.hidden = false;
      clubSlugLabel.textContent = `${t("club.register.pickClubLabel")}: ${activeClubSlug}`;
      show("login");
    }
  } else {
    loginHeading.dataset.i18n = "club.login.pilotTitle";
    loginHeading.textContent = t("club.login.pilotTitle");
    show(step === "pick" ? "pick" : "login");
  }

  document.getElementById("slug-continue")?.addEventListener("click", () => {
    const input = document.getElementById("club-slug") as HTMLInputElement;
    activeClubSlug = input.value.trim().toLowerCase();
    if (!activeClubSlug) return;
    clubSlugLabel.hidden = false;
    clubSlugLabel.textContent = `${t("club.register.pickClubLabel")}: ${activeClubSlug}`;
    show("login");
  });

  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (meRes.ok && mode === "pilot" && step !== "login") {
    show("pick");
    await renderPick();
    return;
  }

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(form));
    const password = String(body.password ?? "");
    if (!password && mode === "pilot") {
      msg.textContent = t("club.login.passwordOrMagic");
      return;
    }
    if (password) {
      const res = await fetch("/api/account/login", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Accept: "application/json",
        },
        credentials: "same-origin",
        body: JSON.stringify({ email: body.email, password, next: nextAfterLogin }),
      });
      if (!res.ok) {
        const err = (await res.json().catch(() => ({}))).error ?? "unauthorized";
        msg.textContent = apiErrorMessage(String(err));
        return;
      }
      msg.textContent = t("club.login.success");
      await afterAccountLogin();
      return;
    }
    await sendMagic(String(body.email));
  });

  document.getElementById("magic-btn")?.addEventListener("click", async () => {
    const email = (form.querySelector('input[name="email"]') as HTMLInputElement).value.trim();
    if (!email) return;
    await sendMagic(email);
  });

  async function sendMagic(email: string) {
    const res = await fetch("/api/account/magic-link", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ email }),
    });
    if (res.ok) {
      msg.textContent = t("club.login.magicSent");
    } else {
      msg.textContent = t("club.login.failed");
    }
  }

  async function afterAccountLogin() {
    if (mode === "club") {
      const ok = await finishClubStaffPath();
      if (!ok) return;
      location.href = nextAfterLogin;
      return;
    }
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) {
      show("login");
      return;
    }
    const data = await me.json();
    const activeId = data.activeMembershipId as string | null | undefined;
    const memberships = (data.memberships ?? []) as Array<{ id: string; status: string }>;
    const activeMem = activeId
      ? memberships.find((m) => m.id === activeId)
      : undefined;
    if (activeMem?.status === "active") {
      location.href = nextAfterLogin;
      return;
    }
    show("pick");
    await renderPick();
  }

  async function finishClubStaffPath(): Promise<boolean> {
    if (!activeClubSlug) {
      show("slug");
      return false;
    }
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) return false;
    const data = await me.json();
    const memberships = (data.memberships ?? []) as Array<{
      id: string;
      slug?: string;
      status: string;
      role?: string;
    }>;
    const match = memberships.find(
      (m) =>
        (m.slug ?? "").toLowerCase() === activeClubSlug &&
        m.status === "active" &&
        staffRoles(m.role ?? ""),
    );
    if (!match) {
      msg.textContent = t("club.login.staffDenied");
      return false;
    }
    const res = await fetch("/api/account/switch-club", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ membershipId: match.id }),
    });
    return res.ok;
  }

  legacy?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(legacy!));
    const res = await fetch("/api/club/login", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      credentials: "same-origin",
      body: JSON.stringify({ ...body, next: nextAfterLogin }),
    });
    if (res.ok) location.href = nextAfterLogin;
  });

  async function renderPick() {
    const list = document.getElementById("club-list") as HTMLUListElement;
    list.innerHTML = "";
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) {
      show("login");
      return;
    }
    const data = await me.json();
    const memberships = (data.memberships ?? []) as Array<{
      id: string;
      name?: string;
      slug?: string;
      status: string;
    }>;
    if (!memberships.length) {
      const li = document.createElement("li");
      li.className = "muted";
      li.textContent = t("club.login.noClubs");
      list.appendChild(li);
    }
    for (const m of memberships) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = `${m.name ?? m.slug ?? m.id} (${m.status})`;
      btn.disabled = m.status !== "active";
      btn.addEventListener("click", async () => {
        const res = await fetch("/api/account/switch-club", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          credentials: "same-origin",
          body: JSON.stringify({ membershipId: m.id }),
        });
        if (res.ok) location.href = nextAfterLogin;
      });
      li.appendChild(btn);
      list.appendChild(li);
    }
  }

  const joinBtn = document.getElementById("join-open");
  joinBtn?.addEventListener("click", () => show("join"));

  joinForm?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(joinForm));
    const res = await fetch("/api/account/join", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      credentials: "same-origin",
      body: JSON.stringify(body),
    });
    const msgJ = document.getElementById("join-msg") as HTMLParagraphElement;
    if (res.ok) {
      msgJ.textContent = t("account.join.pending");
      show("pick");
      await renderPick();
    } else {
      const err = (await res.json().catch(() => ({}))).error ?? "join failed";
      msgJ.textContent = apiErrorMessage(String(err));
    }
  });

  if (meRes.ok && mode === "club") {
    const ok = await finishClubStaffPath();
    if (ok) location.href = nextAfterLogin;
  }
}