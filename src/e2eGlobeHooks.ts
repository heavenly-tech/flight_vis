/** True when Playwright CSP globe tests request the minimal e2e surface (?e2eGlobeHooks=1). */
export function e2eGlobeHooksEnabled(): boolean {
  if (import.meta.env.VITE_E2E_GLOBE_HOOKS === "1") return true;
  return new URLSearchParams(window.location.search).has("e2eGlobeHooks");
}

export type FvE2eSurface = {
  globePainted: boolean;
  getPerfCommands: () => number;
  getAircraftPosition: () => { lon: number; lat: number; alt: number } | null;
};

declare global {
  interface Window {
    __fvE2e?: FvE2eSurface;
  }
}

export function markGlobePainted(): void {
  if (!e2eGlobeHooksEnabled() || !window.__fvE2e) return;
  window.__fvE2e.globePainted = true;
}