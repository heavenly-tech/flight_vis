import { startClubPage } from "./i18nPage";
import { apiErrorMessage, t } from "../i18n/index";
import { safeNextPath } from "./safeNext";

export async function bootClubLoginAccount(): Promise<void> {
  await startClubPage({ persistUser: false });
  const params = new URLSearchParams(location.search);
  const step = params.get("step") ?? "login";
  const nextAfterLogin = safeNextPath(params.get("next") ?? "/club-pilot.html");

  const loginPanel = document.getElementById("login-panel") as HTMLElement;
  const pickPanel = document.getElementById("pick-panel") as HTMLElement;
  const joinPanel = document.getElementById("join-panel") as HTMLElement;

  const show = (panel: "login" | "pick" | "join") => {
    loginPanel.hidden = panel !== "login";
    pickPanel.hidden = panel !== "pick";
    joinPanel.hidden = panel !== "join";
  };

  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (meRes.ok && step !== "login") {
    show("pick");
    await renderPick();
    return;
  }

  if (step === "pick") {
    show("pick");
    await renderPick();
    return;
  }

  show("login");

  const form = document.getElementById("form") as HTMLFormElement;
  const msg = document.getElementById("msg") as HTMLParagraphElement;
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(form));
    const res = await fetch("/api/account/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(body),
    });
    if (res.ok) {
      msg.textContent = t("club.login.success");
      show("pick");
      await renderPick();
    } else {
      const err = (await res.json().catch(() => ({}))).error ?? "unauthorized";
      msg.textContent = apiErrorMessage(String(err));
    }
  });

  const legacy = document.getElementById("legacy-login") as HTMLFormElement;
  legacy.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(legacy));
    const res = await fetch("/api/club/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(body),
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

  const joinForm = document.getElementById("join-form") as HTMLFormElement;
  joinForm?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(joinForm));
    const res = await fetch("/api/account/join", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
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
}