import { startClubPage } from "./i18nPage";
import { t, apiErrorMessage } from "../i18n/index";

function slugFromPath(): string | null {
  const m = location.pathname.match(/^\/c\/([^/]+)\/registro\/?$/);
  if (m) return decodeURIComponent(m[1]);
  const q = new URLSearchParams(location.search).get("club");
  return q?.trim() || null;
}

await startClubPage({ persistUser: false });

const form = document.getElementById("form") as HTMLFormElement;
const msg = document.getElementById("msg")!;
const clubTitle = document.getElementById("club-title");
const clubPick = document.getElementById("club-pick") as HTMLSelectElement | null;

let slug = slugFromPath();

async function loadPublicClubs() {
  if (slug || !clubPick) return;
  const res = await fetch("/api/clubs/public");
  const data = (await res.json().catch(() => ({}))) as { clubs?: { slug: string; name: string }[] };
  const clubs = data.clubs ?? [];
  if (clubs.length === 1) {
    location.replace(`/c/${encodeURIComponent(clubs[0].slug)}/registro`);
    return;
  }
  clubPick.replaceChildren(
    ...clubs.map((c) => {
      const opt = document.createElement("option");
      opt.value = c.slug;
      opt.textContent = c.name;
      return opt;
    }),
  );
  if (clubs.length > 0) {
    slug = clubs[0].slug;
    clubPick.hidden = false;
    document.getElementById("club-pick-wrap")?.removeAttribute("hidden");
  }
}

async function showClubName() {
  if (!slug || !clubTitle) return;
  const res = await fetch(`/api/clubs/${encodeURIComponent(slug)}`, {
    headers: { Accept: "application/json" },
  });
  if (!res.ok) {
    clubTitle.textContent = t("club.register.title");
    return;
  }
  const data = (await res.json()) as { club?: { name?: string } };
  const name = data.club?.name;
  if (name) clubTitle.textContent = t("club.register.forClub", { club: name });
}

clubPick?.addEventListener("change", () => {
  slug = clubPick.value;
  void showClubName();
});

await loadPublicClubs();
if (!slug && clubPick?.value) slug = clubPick.value;
await showClubName();

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const chosen = slug ?? clubPick?.value;
  if (!chosen) {
    msg.textContent = t("club.register.pickClub");
    return;
  }
  const body = Object.fromEntries(new FormData(form));
  const res = await fetch("/api/club/register", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ...body, slug: chosen }),
  });
  msg.textContent = res.ok
    ? t("club.register.success")
    : apiErrorMessage((await res.json().catch(() => ({}))).error ?? "register failed");
});