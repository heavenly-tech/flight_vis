import { startClubPage } from "./i18nPage";
import { t } from "../i18n/index";

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

function localizedRole(role: string): string {
  if (role === "admin") return t("club.admin.roleAdmin");
  if (role === "staff") return t("club.admin.roleStaff");
  return t("club.admin.rolePilot");
}

function localizedPilotStatus(status: string): string {
  if (status === "approved" || status === "active") return t("pilot.statusApproved");
  if (status === "pending") return t("pilot.statusPending");
  return status;
}

function localizedFlightStatus(status: string): string {
  return status === "active" ? t("flight.statusLive") : t("flight.statusReplay");
}

function localizedVisibility(visibility: string): string {
  if (visibility === "public") return t("pilot.visibilityPublic");
  if (visibility === "club") return t("pilot.visibilityClub");
  return t("pilot.visibilityPrivate");
}

async function boot(): Promise<void> {
  try {
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
      who.textContent = t("pilot.notSignedIn");
      const link = document.createElement("a");
      link.href = "/login?mode=pilot";
      link.textContent = t("pilot.logInLink");
      profile.replaceChildren(link);
      return;
    }
    who.textContent = `${me.pilot.name} · ${localizedPilotStatus(me.pilot.status)} · ${localizedRole(me.pilot.role)}`;
    profile.textContent = `${me.pilot.email} · ${me.pilot.licenseNo || "—"}`;
    const devs = await api<{ devices: { id: string; label: string }[] }>("/api/me/devices");
    devices.replaceChildren(
      ...devs.devices.map((d) => {
        const li = document.createElement("li");
        li.textContent = d.label || t("pilot.unnamedDevice");
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
        live.textContent = `${f.tail} · ${localizedFlightStatus(f.status)} · ${localizedVisibility(f.visibility)}`;
        const gpx = document.createElement("a");
        gpx.href = `/api/flights/${f.id}/gpx`;
        gpx.textContent = "GPX";
        gpx.style.marginLeft = ".5rem";
        const del = document.createElement("button");
        del.type = "button";
        del.textContent = t("common.delete");
        del.style.marginLeft = ".5rem";
        del.addEventListener("click", async () => {
          if (!confirm(t("pilot.deleteTrackConfirm"))) return;
          try {
            await api(`/api/me/flights/${encodeURIComponent(f.id)}`, { method: "DELETE" });
            await boot();
          } catch (e) {
            console.error("[pilot] delete flight failed:", e);
            who.textContent = t("api.genericError");
          }
        });
        li.append(live, gpx, del);
        return li;
      }),
    );
  } catch (e) {
    console.error("[pilot] boot failed:", e);
    who.textContent = t("api.genericError");
  }
}

newDeviceBtn.addEventListener("click", async () => {
  try {
    const body = await api<{ id: string }>("/api/me/devices", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ label: labelInput.value.trim() }),
    });
    qr.src = `/api/devices/pair/${encodeURIComponent(body.id)}/qr.svg`;
    tokenReveal.hidden = false;
    await boot();
  } catch (e) {
    console.error("[pilot] create device failed:", e);
    who.textContent = t("api.genericError");
  }
});

logoutBtn?.addEventListener("click", async () => {
  await api("/api/club/logout", { method: "POST" });
  location.href = "/login?mode=pilot";
});

void (async () => {
  await startClubPage();
  await boot();
  const onLocale = () => {
    void boot();
  };
  window.addEventListener("fv-locale-change", onLocale);
  window.addEventListener("fv-i18n-ready", onLocale);
})();
