import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";

const pending = document.querySelector<HTMLUListElement>("#pending")!;
const all = document.querySelector<HTMLUListElement>("#all")!;

async function refresh(): Promise<void> {
  const pend = await api<{ pilots: { id: string; name: string; email: string }[] }>(
    "/api/admin/pilots/pending",
  );
  pending.replaceChildren(
    ...pend.pilots.map((p) => {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.textContent = "Approve";
      btn.addEventListener("click", async () => {
        await api(`/api/admin/pilots/${encodeURIComponent(p.id)}/approve`, { method: "POST" });
        await refresh();
      });
      li.append(`${p.name} (${p.email}) `, btn);
      return li;
    }),
  );
  const roster = await api<{
    pilots: { name: string; email: string; status: string; role: string }[];
  }>("/api/admin/pilots");
  all.replaceChildren(
    ...roster.pilots.map((p) => {
      const li = document.createElement("li");
      li.textContent = `${p.name} · ${p.email} · ${p.status} · ${p.role}`;
      return li;
    }),
  );
}

void (async () => {
  void startClubPage({ switcher: "#lang-slot" });
  if (!(await ensureClubAdminOrRedirect())) return;
  mountAdminNav();
  try {
    await requireAdmin();
    await refresh();
  } catch (e) {
    if (!isClubAdminRedirect(e)) pending.innerHTML = "<li>Admin login required.</li>";
  }
})();