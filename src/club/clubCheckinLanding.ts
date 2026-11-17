import { initI18n, applyI18nDocument, t } from "../i18n/index";

function slugAndToken(): { slug: string; token: string } | null {
  const m = location.pathname.match(/^\/c\/([^/]+)\/q\/([^/]+)/);
  if (!m) return null;
  return { slug: decodeURIComponent(m[1]), token: decodeURIComponent(m[2]) };
}

const pair = slugAndToken();
const intentBtn = document.querySelector<HTMLAnchorElement>("#btn-intent")!;
const mapList = document.querySelector<HTMLUListElement>("#map-list")!;

async function boot(): Promise<void> {
  if (!pair) return;
  const cfgRes = await fetch(
    `/api/c/${encodeURIComponent(pair.slug)}/q/${encodeURIComponent(pair.token)}/landing`,
  );
  let clubDefault: string | null = null;
  if (cfgRes.ok) {
    const cfg = (await cfgRes.json()) as {
      intentUrl: string;
      appPath: string;
      clubDefaultLocale?: string;
      resolvedLocale?: string;
    };
    clubDefault = cfg.clubDefaultLocale ?? null;
    initI18n({
      clubDefaultLocale: cfg.resolvedLocale ?? cfg.clubDefaultLocale,
    });
    applyI18nDocument();
    intentBtn.href = cfg.intentUrl;
    intentBtn.textContent = t("checkin.openApp");
    const dl = document.querySelector<HTMLAnchorElement>("#btn-download");
    if (dl) {
      dl.href = cfg.appPath;
      dl.textContent = t("checkin.downloadApp");
    }
  } else {
    initI18n({ clubDefaultLocale: clubDefault });
    applyI18nDocument();
  }
  const mapRes = await fetch("/api/club/map/public");
  if (mapRes.ok) {
    const data = (await mapRes.json()) as { aircraft?: { tail: string; pilotName?: string }[] };
    mapList.replaceChildren(
      ...(data.aircraft ?? []).map((row) => {
        const li = document.createElement("li");
        const name = row.pilotName ? ` · ${row.pilotName}` : "";
        li.textContent = `${row.tail}${name}`;
        return li;
      }),
    );
  }
}

void boot();