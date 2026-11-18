import { startClubPage } from "./i18nPage";

void (async () => {
  await startClubPage({ persistUser: false });
  const list = document.querySelector<HTMLUListElement>("#list");
  if (!list) return;
  const res = await fetch("/api/club/map/public");
  const data = (await res.json()) as { aircraft?: { tail: string; pilotName?: string }[] };
  list.replaceChildren(
    ...(data.aircraft ?? []).map((row) => {
      const li = document.createElement("li");
      const name = row.pilotName ? ` · ${row.pilotName}` : "";
      li.textContent = `${row.tail}${name}`;
      return li;
    }),
  );
})();