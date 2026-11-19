import { bootI18n, t } from "../i18n/boot";

type AppRelease = {
  available: boolean;
  version: string | null;
  sha256: string | null;
  apkUrl: string;
  message?: string;
};

const status = document.querySelector<HTMLParagraphElement>("#status")!;
const versionEl = document.querySelector<HTMLParagraphElement>("#version")!;
const shaWrap = document.querySelector<HTMLParagraphElement>("#sha")!;
const shaVal = document.querySelector<HTMLElement>("#sha-val")!;
const download = document.querySelector<HTMLAnchorElement>("#download")!;
const openInApp = document.querySelector<HTMLAnchorElement>("#open-in-app")!;
const sideloadSteps = document.querySelector<HTMLOListElement>("#sideload-steps")!;

async function loadRelease(): Promise<void> {
  try {
    const res = await fetch(`${import.meta.env.BASE_URL}app/app-release.json`, { cache: "no-store" });
    if (!res.ok) {
      throw new Error("missing metadata");
    }
    const data = (await res.json()) as AppRelease;
    const apkProbe = await fetch(data.apkUrl, { method: "HEAD" });
    const hasApk = apkProbe.ok && data.available;
    if (!hasApk) {
      status.textContent = data.message ?? t("app.releaseSoon");
      download.hidden = true;
      return;
    }
    status.textContent = t("app.releaseReady");
    if (data.version) {
      versionEl.hidden = false;
      versionEl.textContent = t("app.versionLabel", { version: data.version });
    }
    if (data.sha256) {
      shaWrap.hidden = false;
      shaVal.textContent = data.sha256;
    }
    download.href = data.apkUrl;
    download.hidden = false;
    try {
      const linkRes = await fetch("/api/app/install-link", { credentials: "same-origin" });
      if (linkRes.ok) {
        const link = (await linkRes.json()) as { intentUrl?: string | null; apkUrl?: string };
        if (link.intentUrl?.includes("S.browser_fallback_url=")) {
          openInApp.href = link.intentUrl;
          openInApp.hidden = false;
        }
      }
    } catch {
      openInApp.hidden = true;
    }
  } catch {
    status.textContent = t("app.releaseSoon");
    download.hidden = true;
  }
}

void (async () => {
  await bootI18n({ switcher: "#lang-slot" });
  sideloadSteps.replaceChildren(
    ...[1, 2, 3, 4, 5].map((n) => {
      const li = document.createElement("li");
      li.textContent = t(`app.sideloadStep${n}`);
      return li;
    }),
  );
  await loadRelease();
})();