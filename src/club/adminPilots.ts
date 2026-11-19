import { t } from "../i18n/index";
import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";

const pending = document.querySelector<HTMLUListElement>("#pending")!;
const memberships = document.querySelector<HTMLDivElement>("#memberships")!;
const inviteForm = document.querySelector<HTMLFormElement>("#invite-form")!;
const inviteEmail = document.querySelector<HTMLInputElement>("#invite-email")!;
const inviteStatus = document.querySelector<HTMLParagraphElement>("#invite-status")!;

function roleLabel(role: string): string {
  if (role === "admin") return t("club.admin.roleAdmin");
  if (role === "staff") return t("club.admin.roleStaff");
  return t("club.admin.rolePilot");
}

async function postAction(path: string): Promise<void> {
  const res = await fetch(path, { method: "POST", credentials: "same-origin" });
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    if (body.error === "last_admin") {
      alert(t("club.admin.lastAdmin"));
      return;
    }
    throw new Error(`${res.status}`);
  }
}

async function refresh(): Promise<void> {
  const pend = await api<{ pilots: { id: string; name: string; email: string }[] }>(
    "/api/admin/pilots/pending",
  );
  pending.replaceChildren(
    ...pend.pilots.map((p) => {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.textContent = "Aprobar";
      btn.addEventListener("click", async () => {
        await api(`/api/admin/pilots/${encodeURIComponent(p.id)}/approve`, { method: "POST" });
        await refresh();
      });
      li.append(`${p.name} (${p.email}) `, btn);
      return li;
    }),
  );

  const roster = await api<{
    memberships: {
      id: string;
      name: string;
      email: string;
      status: string;
      role: string;
    }[];
  }>("/api/admin/memberships");

  memberships.replaceChildren(
    ...roster.memberships.map((m) => {
      const row = document.createElement("div");
      row.className = "mem-row";
      const title = document.createElement("div");
      const badge = document.createElement("span");
      badge.className = "badge";
      badge.textContent = `${roleLabel(m.role)} · ${m.status}`;
      title.append(`${m.name} · ${m.email} `, badge);
      row.appendChild(title);

      const actions = document.createElement("div");
      actions.className = "mem-actions";

      if (m.role !== "admin" && m.status === "active") {
        const promote = document.createElement("button");
        promote.type = "button";
        promote.textContent = t("club.admin.promote");
        promote.addEventListener("click", async () => {
          await postAction(`/api/admin/memberships/${encodeURIComponent(m.id)}/promote`);
          await refresh();
        });
        actions.appendChild(promote);
      }
      if (m.role === "admin" && m.status === "active") {
        const demote = document.createElement("button");
        demote.type = "button";
        demote.textContent = t("club.admin.demote");
        demote.addEventListener("click", async () => {
          await postAction(`/api/admin/memberships/${encodeURIComponent(m.id)}/demote`);
          await refresh();
        });
        const remove = document.createElement("button");
        remove.type = "button";
        remove.textContent = t("club.admin.remove");
        remove.addEventListener("click", async () => {
          await postAction(`/api/admin/memberships/${encodeURIComponent(m.id)}/remove`);
          await refresh();
        });
        actions.appendChild(demote);
        actions.appendChild(remove);
      } else if (m.status === "active") {
        const remove = document.createElement("button");
        remove.type = "button";
        remove.textContent = t("club.admin.remove");
        remove.addEventListener("click", async () => {
          await postAction(`/api/admin/memberships/${encodeURIComponent(m.id)}/remove`);
          await refresh();
        });
        actions.appendChild(remove);
      }

      if (actions.childElementCount > 0) {
        row.appendChild(actions);
      }
      return row;
    }),
  );
}

inviteForm.addEventListener("submit", async (ev) => {
  ev.preventDefault();
  inviteStatus.textContent = "";
  const email = inviteEmail.value.trim();
  if (!email) return;
  try {
    await api("/api/admin/memberships/invite", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email }),
    });
    inviteStatus.textContent = t("club.admin.inviteSent");
    inviteEmail.value = "";
  } catch {
    inviteStatus.textContent = t("club.admin.inviteSent");
  }
});

void (async () => {
  if (!(await ensureClubAdminOrRedirect())) return;
  mountAdminNav();
  try {
    await requireAdmin();
    await refresh();
  } catch (e) {
    if (!isClubAdminRedirect(e)) pending.innerHTML = `<li>${t("api.adminOnly")}</li>`;
  }
})();