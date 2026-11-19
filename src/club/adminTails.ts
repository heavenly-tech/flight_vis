import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";

const list = document.querySelector<HTMLUListElement>("#list")!;
const form = document.querySelector<HTMLFormElement>("#form")!;

async function refresh(): Promise<void> {
  const data = await api<{
    aircraft: { id: string; tail: string; type_code: string; active: boolean }[];
  }>("/api/admin/aircraft");
  list.replaceChildren(
    ...data.aircraft.map((a) => {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = a.active ? "Deactivate" : "Activate";
      btn.addEventListener("click", async () => {
        await api("/api/admin/aircraft", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ tail: a.tail, type: a.type_code, active: !a.active }),
        });
        await refresh();
      });
      li.append(`${a.tail} · ${a.type_code} · ${a.active ? "active" : "inactive"} `, btn);
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
  void startClubPage({ switcher: "#lang-slot" });
  if (!(await ensureClubAdminOrRedirect())) return;
  mountAdminNav();
  try {
    await requireAdmin();
    await refresh();
  } catch (e) {
    if (!isClubAdminRedirect(e)) list.innerHTML = "<li>Admin login required.</li>";
  }
})();