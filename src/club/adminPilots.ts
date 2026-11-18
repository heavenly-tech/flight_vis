import { api, mountAdminLangSwitcher, mountAdminNav, requireAdmin } from "./adminCommon";
import { t } from "../i18n/index";

const pending = document.querySelector<HTMLUListElement>("#pending")!;
const all = document.querySelector<HTMLUListElement>("#all")!;
const form = document.querySelector<HTMLFormElement>("#add-form")!;

type PilotRow = {
  id: string;
  name: string;
  email: string;
  license_no: string;
  status: string;
  role: string;
};

async function refresh(): Promise<void> {
  const pend = await api<{ pilots: { id: string; name: string; email: string }[] }>(
    "/api/admin/pilots/pending",
  );
  pending.replaceChildren(
    ...pend.pilots.map((p) => {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = t("admin.roster.approve");
      btn.addEventListener("click", async () => {
        await api(`/api/admin/pilots/${encodeURIComponent(p.id)}/approve`, { method: "POST" });
        await refresh();
      });
      li.append(`${p.name} (${p.email}) `, btn);
      return li;
    }),
  );
  const roster = await api<{ pilots: PilotRow[] }>("/api/admin/pilots");
  all.replaceChildren(
    ...roster.pilots.map((p) => {
      const li = document.createElement("li");
      li.append(`${p.name} · ${p.email} · ${p.status} · ${p.role} `);
      const edit = document.createElement("button");
      edit.type = "button";
      edit.textContent = t("admin.roster.edit");
      edit.addEventListener("click", async () => {
        const name = window.prompt(t("admin.roster.namePrompt"), p.name);
        if (name === null) return;
        const email = window.prompt(t("admin.roster.emailPrompt"), p.email);
        if (email === null) return;
        const license = window.prompt(t("admin.roster.licensePrompt"), p.license_no) ?? p.license_no;
        const status =
          window.prompt(t("admin.roster.statusPrompt"), p.status) ?? p.status;
        await api(`/api/admin/pilots/${encodeURIComponent(p.id)}`, {
          method: "PATCH",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            name: name.trim(),
            email: email.trim(),
            license_no: license.trim(),
            status: status.trim(),
          }),
        });
        await refresh();
      });
      const del = document.createElement("button");
      del.type = "button";
      del.textContent = t("admin.roster.remove");
      del.addEventListener("click", async () => {
        if (!window.confirm(t("admin.roster.removeConfirm"))) return;
        await api(`/api/admin/pilots/${encodeURIComponent(p.id)}`, { method: "DELETE" });
        await refresh();
      });
      li.append(edit, " ", del);
      return li;
    }),
  );
}

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(form);
  await api("/api/admin/pilots", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      name: String(fd.get("name")).trim(),
      email: String(fd.get("email")).trim(),
      license_no: String(fd.get("license_no") || "").trim(),
      status: String(fd.get("status") || "approved").trim(),
    }),
  });
  form.reset();
  await refresh();
});

void (async () => {
  mountAdminNav();
  mountAdminLangSwitcher();
  try {
    await requireAdmin();
    await refresh();
  } catch {
    all.innerHTML = `<li>${t("api.adminOnly")}</li>`;
  }
})();