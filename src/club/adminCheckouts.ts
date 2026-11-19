import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";

const list = document.querySelector<HTMLUListElement>("#list")!;
const form = document.querySelector<HTMLFormElement>("#form")!;
const pilots = document.querySelector<HTMLSelectElement>("#pilot")!;

async function refresh(): Promise<void> {
  const roster = await api<{ pilots: { id: string; name: string; email: string }[] }>(
    "/api/admin/pilots",
  );
  pilots.replaceChildren(
    ...roster.pilots.map((p) => {
      const opt = document.createElement("option");
      opt.value = p.id;
      opt.textContent = `${p.name} (${p.email})`;
      return opt;
    }),
  );
  const data = await api<{
    checkouts: {
      pilot_name: string;
      type_code: string;
      tail: string | null;
      expires_at: number | null;
    }[];
  }>("/api/admin/checkouts");
  list.replaceChildren(
    ...data.checkouts.map((c) => {
      const li = document.createElement("li");
      const exp = c.expires_at ? new Date(c.expires_at).toISOString() : "no expiry";
      const tail = c.tail ?? "all tails of type";
      li.textContent = `${c.pilot_name} · ${c.type_code} · ${tail} · ${exp}`;
      return li;
    }),
  );
}

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const fd = new FormData(form);
  const expiresRaw = String(fd.get("expires_at") || "").trim();
  const expires_at = expiresRaw ? Date.parse(expiresRaw) : undefined;
  await api("/api/admin/checkouts", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      pilot_id: pilots.value,
      type: String(fd.get("type")).trim(),
      tail: String(fd.get("tail") || "").trim() || undefined,
      expires_at,
    }),
  });
  await refresh();
});

void (async () => {
  await startClubPage({ switcher: "#lang-slot" });
  if (!(await ensureClubAdminOrRedirect())) return;
  mountAdminNav();
  try {
    await requireAdmin();
    await refresh();
  } catch (e) {
    if (!isClubAdminRedirect(e)) list.innerHTML = "<li>Admin login required.</li>";
  }
})();