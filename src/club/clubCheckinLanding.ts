import { initI18n, applyI18nDocument, t } from "../i18n/index";
import { fetchAppRelease, renderReleaseInfo } from "../app/releaseInfo";

type LandingConfig = {
  intentUrl?: string;
  clubName?: string;
  logoUrl?: string | null;
  slug?: string;
  clubDefaultLocale?: string;
  resolvedLocale?: string;
  releaseAvailable?: boolean;
  apkUrl?: string;
  publicMapPath?: string;
};

function landingPaths(): { slug: string; token: string | null } | null {
  const withToken = location.pathname.match(/^\/c\/([^/]+)\/q\/([^/]+)/);
  if (withToken) {
    return {
      slug: decodeURIComponent(withToken[1]),
      token: decodeURIComponent(withToken[2]),
    };
  }
  const slugOnly = location.pathname.match(/^\/c\/([^/]+)\/?$/);
  if (slugOnly) {
    return { slug: decodeURIComponent(slugOnly[1]), token: null };
  }
  return null;
}

const paths = landingPaths();
const intentBtn = document.querySelector<HTMLAnchorElement>("#btn-intent")!;
const mapLink = document.querySelector<HTMLAnchorElement>("#view-live-map")!;
const clubNameEl = document.querySelector<HTMLHeadingElement>("#club-name")!;
const logoEl = document.querySelector<HTMLImageElement>("#club-logo")!;
const downloadSlot = document.querySelector<HTMLDivElement>("#download-primary")!;
const androidSteps = document.querySelector<HTMLOListElement>("#android-steps")!;
const appVersionEl = document.querySelector<HTMLParagraphElement>("#app-version")!;
const appReleaseNoteEl = document.querySelector<HTMLParagraphElement>("#app-release-note")!;

function renderDownload(available: boolean, apkUrl: string): void {
  downloadSlot.replaceChildren();
  if (available) {
    const a = document.createElement("a");
    a.className = "dl-primary";
    a.id = "btn-download-primary";
    a.href = apkUrl;
    a.setAttribute("download", "");
    a.textContent = t("checkin.downloadApp");
    downloadSlot.appendChild(a);
  } else {
    const span = document.createElement("span");
    span.className = "coming-soon";
    span.id = "btn-download-primary";
    span.textContent = t("checkin.comingSoon");
    downloadSlot.appendChild(span);
  }
}

function renderAndroidSteps(): void {
  androidSteps.replaceChildren(
    ...[1, 2, 3].map((n) => {
      const li = document.createElement("li");
      li.textContent = t(`checkin.androidStep${n}`);
      return li;
    }),
  );
}

async function loadConfig(): Promise<void> {
  if (!paths) return;
  const cfgUrl = paths.token
    ? `/api/c/${encodeURIComponent(paths.slug)}/q/${encodeURIComponent(paths.token)}/landing`
  : `/api/c/${encodeURIComponent(paths.slug)}/qr-landing`;
  const cfgRes = await fetch(cfgUrl);
  let cfg: LandingConfig = {};
  if (cfgRes.ok) {
    cfg = (await cfgRes.json()) as LandingConfig;
  }
  initI18n({
    clubDefaultLocale: cfg.resolvedLocale ?? cfg.clubDefaultLocale,
  });
  applyI18nDocument();
  if (cfg.clubName) {
    clubNameEl.textContent = cfg.clubName;
  }
  if (cfg.logoUrl) {
    logoEl.src = cfg.logoUrl;
    logoEl.hidden = false;
    logoEl.alt = cfg.clubName ?? "";
  }
  if (cfg.intentUrl) {
    intentBtn.href = cfg.intentUrl;
    intentBtn.textContent = t("checkin.openApp");
  } else if (!paths.token) {
    intentBtn.hidden = true;
  }
  mapLink.href = cfg.publicMapPath ?? "/public-map.html";
  mapLink.textContent = t("checkin.viewLiveMap");
  
  // Try to fetch app release info
  const appRelease = await fetchAppRelease();
  
  // Determine which release info to use
  let releaseAvailable = false;
  let apkUrl = "/app/flightvis.apk";
  
  if (appRelease) {
    // Use app release info if available
    renderReleaseInfo(
      { version: appVersionEl, note: appReleaseNoteEl },
      appRelease,
      (v: string) => t("checkin.versionLabel", { version: v }),
    );
    releaseAvailable = appRelease.available && appRelease.apkUrl.length > 0;
    apkUrl = appRelease.apkUrl;
  } else {
    // Fall back to landing config
    releaseAvailable =
      cfg.releaseAvailable === true && typeof cfg.apkUrl === "string" && cfg.apkUrl.length > 0;
    apkUrl = cfg.apkUrl ?? "/app/flightvis.apk";
  }
  
  renderDownload(releaseAvailable, apkUrl);
  renderAndroidSteps();
  const reg = document.querySelector<HTMLAnchorElement>("#link-register");
  if (reg && paths.slug) {
    reg.href = `/c/${encodeURIComponent(paths.slug)}/registro`;
  }
}

void loadConfig();