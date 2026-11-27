import "cesium/Build/Cesium/Widgets/widgets.css";
import "./embed3d.css";
import { e2eGlobeHooksEnabled } from "../e2eGlobeHooks";
import { applyI18nDocument, initI18n, t } from "../i18n/index";
import { createE2eMultiSceneStub } from "../viz/e2eMultiSceneStub";
import type { MultiSceneHandle } from "../viz/multiScene";
import {
  BRIDGE_PROTOCOL_VERSION,
  PERF_EMIT_INTERVAL_MS,
  createPerfEmitter,
  emitBridgeEvent,
  parseBridgeMessage,
  type BridgeInboundMessage,
  type BridgeLang,
} from "./bridge";

declare const __FV_BUILD_ID__: string;

export type FvEmbedBridgeApi = {
  receive: (msg: unknown) => boolean;
};

export type FvEmbedE2eSurface = {
  readonly flightCount: number;
  pointCount: (id: string) => number;
  readonly following: boolean;
  readonly painted: boolean;
};

declare global {
  interface Window {
    fvBridge?: FvEmbedBridgeApi;
    FvBridge?: {
      postMessage?: (message: string) => void;
    };
    __fvEmbed?: FvEmbedE2eSurface;
  }
}

const BUILD_VERSION =
  typeof __FV_BUILD_ID__ === "string" && __FV_BUILD_ID__.length > 0 ? __FV_BUILD_ID__ : "dev";

function hardenCreditLinks(creditsEl: HTMLElement): () => void {
  const patch = () => {
    creditsEl.querySelectorAll<HTMLAnchorElement>("a").forEach((a) => {
      if (a.target !== "_blank") a.target = "_blank";
      if (a.rel !== "noopener noreferrer") a.rel = "noopener noreferrer";
    });
  };
  patch();
  const observer = new MutationObserver(patch);
  observer.observe(creditsEl, { childList: true, subtree: true });
  return () => observer.disconnect();
}

function initialLangFromDocument(): BridgeLang {
  const docLang = (document.documentElement.lang || "").toLowerCase();
  return docLang.startsWith("en") ? "en" : "es";
}

function setStatus(el: HTMLElement, text: string | null, isError = false): void {
  el.classList.toggle("is-error", isError);
  if (!text) {
    el.hidden = true;
    el.textContent = "";
    return;
  }
  el.hidden = false;
  el.textContent = text;
}

async function boot(): Promise<void> {
  const containerEl = document.getElementById("cesiumContainer");
  const creditsNode = document.getElementById("credits");
  const statusNode = document.getElementById("status");
  if (!containerEl || !creditsNode || !statusNode) return;
  const container: HTMLElement = containerEl;
  const creditsEl: HTMLElement = creditsNode;
  const statusEl: HTMLElement = statusNode;

  // Initialize i18n strictly from in-memory locale (never localStorage, cookies, or /api/*).
  initI18n({ stored: initialLangFromDocument() });
  applyI18nDocument();
  setStatus(statusEl, t("embed3d.loading"));

  const stopCreditObserver = hardenCreditLinks(creditsEl);

  let scene: MultiSceneHandle | null = null;
  let webglFailed = false;
  let paused = false;
  const pendingMessages: BridgeInboundMessage[] = [];

  const perfEmitter = createPerfEmitter(
    (ev) => emitBridgeEvent(ev),
    PERF_EMIT_INTERVAL_MS,
  );

  function applyMessage(message: BridgeInboundMessage): void {
    switch (message.type) {
      case "init": {
        initI18n({ stored: message.lang });
        applyI18nDocument();
        if (webglFailed) {
          setStatus(statusEl, t("embed3d.webglError"), true);
        } else if (!scene) {
          setStatus(statusEl, t("embed3d.loading"));
        }
        if (scene) {
          if (message.ownFlightId !== undefined) {
            scene.setOwnFlightId(message.ownFlightId);
          }
          if (message.units !== undefined) {
            scene.setUnits(message.units);
          }
          scene.refreshLocale();
        }
        break;
      }
      case "flights":
        scene?.applyFlights(message);
        break;
      case "follow":
        scene?.setFollow(message.on, message.flightId);
        break;
      case "recenter":
        scene?.recenter();
        break;
      case "overview":
        scene?.overview();
        break;
      case "pause":
        paused = true;
        scene?.setPaused(true);
        break;
      case "resume":
        paused = false;
        scene?.setPaused(false);
        break;
    }
  }

  function receive(raw: unknown): boolean {
    const parsed = parseBridgeMessage(raw);
    if (!parsed.ok) {
      emitBridgeEvent({ type: "error", code: parsed.code });
      return false;
    }
    if (!scene && !webglFailed) {
      if (parsed.message.type === "init") {
        initI18n({ stored: parsed.message.lang });
        applyI18nDocument();
        setStatus(statusEl, t("embed3d.loading"));
      }
      pendingMessages.push(parsed.message);
      return true;
    }
    applyMessage(parsed.message);
    return true;
  }

  window.fvBridge = { receive };

  if (e2eGlobeHooksEnabled()) {
    window.__fvEmbed = {
      get flightCount() {
        return scene ? scene.flightCount() : 0;
      },
      pointCount(id: string) {
        return scene ? scene.pointCount(id) : 0;
      },
      get following() {
        return scene ? scene.isFollowing() : false;
      },
      get painted() {
        return scene ? scene.isPainted() : false;
      },
    };
  }

  const callbacks = {
    onFollowChanged: (ev: { on: false; reason: "gesture" }) => {
      emitBridgeEvent({ type: "followChanged", on: ev.on, reason: ev.reason });
    },
    onPerf: (quality: number) => {
      perfEmitter.maybeEmit(quality, paused);
    },
  };

  const skipGlobe = import.meta.env.VITE_E2E_SKIP_GLOBE === "1";

  try {
    if (skipGlobe) {
      scene = createE2eMultiSceneStub(container, callbacks);
    } else {
      const mod = await import("../viz/multiScene");
      scene = await mod.createMultiScene(container, {
        creditContainer: creditsEl,
        msaaSamples: 2,
        maxResolutionScale: 1.5,
        ...callbacks,
      });
    }
  } catch {
    webglFailed = true;
    setStatus(statusEl, t("embed3d.webglError"), true);
    emitBridgeEvent({ type: "error", code: "webgl_unavailable" });
    return;
  }

  setStatus(statusEl, null);

  for (const queued of pendingMessages.splice(0)) {
    applyMessage(queued);
  }

  const onResize = () => {
    scene?.resize();
  };
  window.addEventListener("resize", onResize);

  window.addEventListener("pagehide", () => {
    window.removeEventListener("resize", onResize);
    stopCreditObserver();
    scene?.destroy();
    scene = null;
  });

  emitBridgeEvent({
    type: "ready",
    protocol: BRIDGE_PROTOCOL_VERSION,
    version: BUILD_VERSION,
  });
}

void boot();
