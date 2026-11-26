import "./openKioskLink.css";
import { api } from "./adminCommon";
import { t } from "../i18n/index";

export type OpenKioskLinkController = {
  updateSlug: (slug: string | null | undefined) => void;
  getHref: () => string | null;
};

export function clubScreenPath(clubSlug: string | null | undefined): string | null {
  if (typeof clubSlug !== "string") return null;
  const trimmed = clubSlug.trim();
  if (!trimmed) return null;
  return `/c/${encodeURIComponent(trimmed)}/screen`;
}

function renderLink(slot: HTMLElement, href: string | null): void {
  if (!href) {
    slot.replaceChildren();
    return;
  }
  const existing = slot.querySelector<HTMLAnchorElement>("a[data-open-kiosk-link]");
  const link = existing ?? document.createElement("a");
  link.className = "btn-secondary open-kiosk-link";
  link.setAttribute("data-open-kiosk-link", "");
  link.href = href;
  link.target = "_blank";
  link.rel = "noopener";
  link.textContent = t("adminMap.openClubScreen");
  if (!existing) {
    slot.replaceChildren(link);
  }
}

export async function mountOpenKioskLink(
  slot: HTMLElement | null,
  opts?: { clubSlug?: string | null },
): Promise<OpenKioskLinkController> {
  let currentHref: string | null = clubScreenPath(opts?.clubSlug);

  if (!slot) {
    return {
      updateSlug: (slug) => {
        currentHref = clubScreenPath(slug);
      },
      getHref: () => currentHref,
    };
  }

  const paint = () => renderLink(slot, currentHref);

  if (!slot.dataset.openKioskLocaleBound) {
    slot.dataset.openKioskLocaleBound = "1";
    const onLocale = () => {
      const existing = slot.querySelector<HTMLAnchorElement>("a[data-open-kiosk-link]");
      if (existing) existing.textContent = t("adminMap.openClubScreen");
    };
    window.addEventListener("fv-locale-change", onLocale);
    window.addEventListener("fv-i18n-ready", onLocale);
  }

  if (!currentHref && opts?.clubSlug === undefined) {
    try {
      const me = await api<{ clubSlug?: string | null; email_verified?: boolean }>("/api/club/me");
      currentHref = clubScreenPath(me.clubSlug);
    } catch {
      currentHref = null;
    }
  }

  paint();

  return {
    updateSlug: (slug) => {
      currentHref = clubScreenPath(slug);
      paint();
    },
    getHref: () => currentHref,
  };
}
