export {};

const who = document.querySelector<HTMLParagraphElement>("#who")!;
const profile = document.querySelector<HTMLParagraphElement>("#profile")!;
const tokenReveal = document.querySelector<HTMLDivElement>("#token-reveal")!;
const qr = document.querySelector<HTMLImageElement>("#qr")!;
const devices = document.querySelector<HTMLUListElement>("#devices")!;
const flights = document.querySelector<HTMLUListElement>("#flights")!;
const labelInput = document.querySelector<HTMLInputElement>("#dev-label")!;
const newDeviceBtn = document.querySelector<HTMLButtonElement>("#btn-new-device")!;
const logoutBtn = document.querySelector<HTMLButtonElement>("#btn-logout")!;

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: "same-origin", ...init });
  if (!res.ok) throw new Error(`${res.status}`);
  return (await res.json()) as T;
}

async function boot(): Promise<void> {
  const me = await api<{
    pilot: {
      name: string;
      status: string;
      role: string;
      email: string;
      licenseNo: string;
    } | null;
  }>("/api/club/me");
  if (!me.pilot) {
    who.textContent = "Not signed in.";
    profile.textContent = "Log in at /club-login.html";
    return;
  }
  who.textContent = `${me.pilot.name} · ${me.pilot.status} · ${me.pilot.role}`;
  profile.textContent = `Email: ${me.pilot.email} · Licence: ${me.pilot.licenseNo || "—"}`;
  const devs = await api<{ devices: { id: string; label: string }[] }>("/api/me/devices");
  devices.replaceChildren(
    ...devs.devices.map((d) => {
      const li = document.createElement("li");
      li.textContent = d.label || d.id;
      return li;
    }),
  );
  const list = await api<{
    flights: { id: string; tail: string; status: string; visibility: string }[];
  }>("/api/me/flights");
  flights.replaceChildren(
    ...list.flights.map((f) => {
      const li = document.createElement("li");
      const live = document.createElement("a");
      live.href = `/club-flight.html?id=${encodeURIComponent(f.id)}`;
      live.textContent = `${f.tail} · ${f.status} · ${f.visibility}`;
      const gpx = document.createElement("a");
      gpx.href = `/api/flights/${f.id}/gpx`;
      gpx.textContent = "GPX";
      gpx.style.marginLeft = ".5rem";
      const del = document.createElement("button");
      del.type = "button";
      del.textContent = "Delete";
      del.style.marginLeft = ".5rem";
      del.addEventListener("click", async () => {
        if (!confirm("Delete this track?")) return;
        await api(`/api/me/flights/${encodeURIComponent(f.id)}`, { method: "DELETE" });
        await boot();
      });
      li.append(live, gpx, del);
      return li;
    }),
  );
}

newDeviceBtn.addEventListener("click", async () => {
  const body = await api<{ id: string }>("/api/me/devices", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label: labelInput.value.trim() }),
  });
  qr.src = `/api/devices/pair/${encodeURIComponent(body.id)}/qr.svg`;
  tokenReveal.hidden = false;
  await boot();
});

logoutBtn?.addEventListener("click", async () => {
  await api("/api/club/logout", { method: "POST" });
  location.href = "/club-login.html";
});

void boot();