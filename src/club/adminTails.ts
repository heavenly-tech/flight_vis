import { api, mountAdminLangSwitcher, mountAdminNav, requireAdmin } from "./adminCommon";
import { t } from "../i18n/index";

const list = document.querySelector<HTMLUListElement>("#list")!;
const form = document.querySelector<HTMLFormElement>("#form")!;

async function refresh(): Promise<void> {
  const data = await api<{
    aircraft: { id: string; tail: string; type_code: string; active: boolean }[];
  }>("/api/admin/aircraft");
  list.replaceChildren(
    ...data.aircraft.map((a) => {
      const li = document.createElement("li");
      const edit = document.createElement("button");
      edit.type = "button";
      edit.textContent = t("admin.roster.edit");
      edit.addEventListener("click", async () => {
        const tail = window.prompt(t("admin.roster.tailPrompt"), a.tail);
        if (tail === null) return;
        const type = window.prompt(t("admin.roster.typePrompt"), a.type_code);
        if (type === null) return;
        await api(`/api/admin/aircraft/${encodeURIComponent(a.id)}`, {
          method: "PATCH",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ tail: tail.trim(), type: type.trim(), active: a.active }),
        });
        await refresh();
      });
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.textContent = a.active ? t("admin.roster.deactivate") : t("admin.roster.activate");
      toggle.addEventListener("click", async () => {
        await api(`/api/admin/aircraft/${encodeURIComponent(a.id)}`, {
          method: "PATCH",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            tail: a.tail,
            type: a.type_code,
            active: !a.active,
          }),
        });
        await refresh();
      });
      const del = document.createElement("button");
      del.type = "button";
      del.textContent = t("admin.roster.remove");
      del.addEventListener("click", async () => {
        if (!window.confirm(t("admin.roster.removeConfirm"))) return;
        await api(`/api/admin/aircraft/${encodeURIComponent(a.id)}`, { method: "DELETE" });
        await refresh();
      });
      li.append(`${a.tail} · ${a.type_code} · ${a.active ? "active" : "inactive"} `, edit, " ", toggle, " ", del);
      return li;
    }),
  );
}

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(form);
  await api("/api/admin/aircraft", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      tail: String(fd.get("tail")).trim(),
      type: String(fd.get("type")).trim(),
      active: true,
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
    list.innerHTML = `<li>${t("api.adminOnly")}</li>`;
  }
})();