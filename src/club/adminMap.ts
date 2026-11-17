import { api, mountAdminNav, requireAdmin } from "./adminCommon";

const list = document.querySelector<HTMLUListElement>("#live")!;
const ticks = document.querySelector<HTMLDivElement>("#ticks")!;
const save = document.querySelector<HTMLButtonElement>("#save")!;
const kiosk = document.querySelector<HTMLButtonElement>("#btn-kiosk")!;
const kioskOut = document.querySelector<HTMLParagraphElement>("#kiosk-url")!;

async function refresh(): Promise<void> {
  const aircraft = await api<{ aircraft: { tail: string }[] }>("/api/admin/aircraft");
  const selected = await api<{ tails: string[] }>("/api/admin/map/tails");
  const set = new Set(selected.tails);
  ticks.replaceChildren(
    ...aircraft.aircraft.map((a) => {
      const label = document.createElement("label");
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.value = a.tail;
      cb.checked = set.size === 0 || set.has(a.tail);
      label.append(cb, document.createTextNode(` ${a.tail}`));
      return label;
    }),
  );
  const live = await api<{ aircraft: { tail: string; pilotName?: string }[] }>("/api/admin/map");
  list.replaceChildren(
    ...live.aircraft.map((row) => {
      const li = document.createElement("li");
      li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
      return li;
    }),
  );
}

kiosk?.addEventListener("click", async () => {
  const body = await api<{ screenUrl: string; token: string }>("/api/admin/display-sessions", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label: "Kiosk" }),
  });
  const url = `${location.origin}${body.screenUrl}`;
  kioskOut.innerHTML = `Open on the kiosk: <a href="${url}" target="_blank" rel="noopener">${url}</a><br />
    Activation token (paste on that page, not in the URL): <code>${body.token}</code>`;
});

save.addEventListener("click", async () => {
  const tails = [...ticks.querySelectorAll<HTMLInputElement>("input:checked")].map((i) => i.value);
  await api("/api/admin/map/tails", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ tails }),
  });
  await refresh();
});

void (async () => {
  mountAdminNav();
  try {
    await requireAdmin();
    await refresh();
    setInterval(() => void refresh(), 5000);
  } catch {
    list.innerHTML = "<li>Admin login required.</li>";
  }
})();