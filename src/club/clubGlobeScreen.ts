export {};

import { mountClubDisplayScreen } from "./clubDisplayCore";

mountClubDisplayScreen({
  statusEl: document.querySelector<HTMLParagraphElement>("#status-bar")!,
  qr: document.querySelector<HTMLImageElement>("#qr")!,
  qrWrap: document.querySelector<HTMLDivElement>("#qr-wrap")!,
  mapLayer: document.querySelector<HTMLElement>("#map-layer")!,
  map: document.querySelector<HTMLUListElement>("#map")!,
  viewport: document.querySelector<HTMLDivElement>("#viewport")!,
  activatePanel: document.querySelector<HTMLDivElement>("#activate")!,
  activateToken: document.querySelector<HTMLInputElement>("#activate-token")!,
  activateBtn: document.querySelector<HTMLButtonElement>("#btn-activate")!,
  localToggleBtn: document.querySelector<HTMLButtonElement>("#btn-local-qr") ?? undefined,
});