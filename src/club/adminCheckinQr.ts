import { isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { ADMIN_PHONE_QR_TIMEOUT_MS } from "./adminPhoneQr";
import { CHECKIN_STEP_SEC } from "./checkinStep";
import { startClubPage } from "./i18nPage";
import { t } from "../i18n/index";

const qrPanel = document.querySelector<HTMLDivElement>("#qr-panel")!;
const qrImg = document.querySelector<HTMLImageElement>("#qr-img")!;
const btnShow = document.querySelector<HTMLButtonElement>("#btn-show-qr")!;
const btnHide = document.querySelector<HTMLButtonElement>("#btn-hide-qr")!;
const status = document.querySelector<HTMLParagraphElement>("#status")!;

declare global {
  interface Window {
    __fvClubCheckinQrBoot?: string;
  }
}

const CONSUME_POLL_GRACE_MS = 8_000;

let visible = false;
let shownAtMs = 0;
let hideTimer: number | undefined;
let stepTimer: number | undefined;
let consumeGraceTimer: number | undefined;
let consumePollTimer: number | undefined;

function setVisible(next: boolean): void {
  visible = next;
  qrPanel.hidden = !visible;
  btnShow.hidden = visible;
  btnHide.hidden = !visible;
  if (!visible) {
    window.clearTimeout(hideTimer);
    window.clearInterval(stepTimer);
    window.clearTimeout(consumeGraceTimer);
    window.clearInterval(consumePollTimer);
    qrImg.removeAttribute("src");
    status.textContent = "";
    return;
  }
  shownAtMs = Date.now();
  refreshQr();
  status.textContent = t("admin.checkinQrRotateHint", { seconds: String(CHECKIN_STEP_SEC) });
  stepTimer = window.setInterval(() => refreshQr(), CHECKIN_STEP_SEC * 1000);
  hideTimer = window.setTimeout(() => setVisible(false), ADMIN_PHONE_QR_TIMEOUT_MS);
  consumeGraceTimer = window.setTimeout(() => {
    void watchConsumed();
    consumePollTimer = window.setInterval(() => void watchConsumed(), 2000);
  }, CONSUME_POLL_GRACE_MS);
}

function refreshQr(): void {
  if (!visible) return;
  const url = `/api/admin/checkin-qr.svg?t=${Date.now()}`;
  qrImg.src = url;
}

async function watchConsumed(): Promise<void> {
  if (!visible || shownAtMs <= 0) return;
  if (Date.now() - shownAtMs < CONSUME_POLL_GRACE_MS) return;
  try {
    const res = await fetch(
      `/api/admin/checkin-qr/consumed?sinceMs=${encodeURIComponent(String(shownAtMs))}`,
      { credentials: "same-origin" },
    );
    if (!res.ok) return;
    const data = (await res.json()) as { consumed?: boolean };
    if (data.consumed) {
      status.textContent = t("admin.checkinQrAlreadyUsed");
      setVisible(false);
    }
  } catch {
    /* ignore */
  }
}

window.__fvClubCheckinQrBoot = "script";

btnShow.addEventListener("click", () => setVisible(true));
btnHide.addEventListener("click", () => setVisible(false));
btnShow.disabled = true;

void (async () => {
  try {
    void startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    window.__fvClubCheckinQrBoot = "gate";
    await requireAdmin();
    btnShow.removeAttribute("disabled");
    btnShow.disabled = false;
    document.body.dataset.checkinQrReady = "1";
    window.__fvClubCheckinQrBoot = "ready";
  } catch (e) {
    window.__fvClubCheckinQrBoot = isClubAdminRedirect(e) ? "redirect" : "error";
    if (!isClubAdminRedirect(e)) status.textContent = t("admin.adminRequired");
  }
})();