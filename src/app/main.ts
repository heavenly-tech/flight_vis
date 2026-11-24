import { bootI18n, t } from "../i18n/boot";
import { fetchAppRelease, renderReleaseInfo } from "./releaseInfo";
import { renderSafeInlineMarkdown } from "./safeMarkdown";

const status = document.querySelector<HTMLParagraphElement>("#status")!;
const releaseNote = document.querySelector<HTMLParagraphElement>("#release-note")!;
const versionEl = document.querySelector<HTMLParagraphElement>("#version")!;
const shaWrap = document.querySelector<HTMLParagraphElement>("#sha")!;
const shaVal = document.querySelector<HTMLElement>("#sha-val")!;
const download = document.querySelector<HTMLAnchorElement>("#download")!;
const openInApp = document.querySelector<HTMLAnchorElement>("#open-in-app")!;
const uninstallNoticeBody = document.querySelector<HTMLParagraphElement>("#uninstall-notice-body");
const sideloadSteps = document.querySelector<HTMLOListElement>("#sideload-steps")!;

async function loadRelease(): Promise<void> {
  try {
    const data = await fetchAppRelease();
    if (!data) {
      throw new Error("missing metadata");
    }
    renderReleaseInfo(
      { version: versionEl, note: releaseNote },
      data,
      (v: string) => t("app.versionLabel", { version: v }),
    );
    const apkProbe = await fetch(data.apkUrl, { method: "HEAD" });
    const hasApk = apkProbe.ok && data.available;
    if (!hasApk) {
      status.textContent = t("app.releaseSoon");
      download.hidden = true;
      return;
    }
    status.textContent = t("app.releaseReady");
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
        if (
          link.intentUrl?.startsWith("intent:#Intent;action=android.intent.action.MAIN;") &&
          link.intentUrl.includes("S.browser_fallback_url=")
        ) {
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
  if (uninstallNoticeBody) {
    uninstallNoticeBody.replaceChildren(renderSafeInlineMarkdown(t("app.uninstallNotice")));
  }
  sideloadSteps.replaceChildren(
    ...[1, 2, 3, 4].map((n) => {
      const li = document.createElement("li");
      li.appendChild(renderSafeInlineMarkdown(t(`app.sideloadStep${n}`)));
      return li;
    }),
  );
  await loadRelease();
})();