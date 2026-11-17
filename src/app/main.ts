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
      status.textContent = data.message ?? "App coming soon";
      download.hidden = true;
      return;
    }
    status.textContent = "Release ready to install.";
    if (data.version) {
      versionEl.hidden = false;
      versionEl.textContent = `Version ${data.version}`;
    }
    if (data.sha256) {
      shaWrap.hidden = false;
      shaVal.textContent = data.sha256;
    }
    download.href = data.apkUrl;
    download.hidden = false;
  } catch {
    status.textContent = "App coming soon";
    download.hidden = true;
  }
}

void loadRelease();