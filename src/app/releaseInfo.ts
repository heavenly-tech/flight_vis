import { releaseNoteText } from "./releaseNote";

export type AppRelease = {
  available: boolean;
  version: string | null;
  sha256: string | null;
  apkUrl: string;
  message?: string;
};

export async function fetchAppRelease(): Promise<AppRelease | null> {
  try {
    const res = await fetch("/app/app-release.json", { cache: "no-store" });
    if (!res.ok) {
      return null;
    }
    const data = (await res.json()) as unknown;
    if (
      typeof data === "object" &&
      data !== null &&
      typeof (data as Record<string, unknown>).available === "boolean" &&
      (typeof (data as Record<string, unknown>).version === "string" ||
        (data as Record<string, unknown>).version === null) &&
      (typeof (data as Record<string, unknown>).sha256 === "string" ||
        (data as Record<string, unknown>).sha256 === null) &&
      typeof (data as Record<string, unknown>).apkUrl === "string"
    ) {
      return data as AppRelease;
    }
    return null;
  } catch {
    return null;
  }
}

export function renderReleaseInfo(
  els: { version: HTMLElement; note: HTMLElement },
  data: AppRelease,
  versionLabel: (v: string) => string,
): void {
  // Version
  if (data.version && typeof data.version === "string" && data.version.trim() !== "") {
    els.version.textContent = versionLabel(data.version);
    els.version.hidden = false;
  } else {
    els.version.hidden = true;
  }

  // Release note
  const note = releaseNoteText(data);
  if (note !== null) {
    els.note.textContent = note;
    els.note.hidden = false;
  } else {
    els.note.hidden = true;
  }
}
