import {
  applyFlightsUpdate,
  type FlightsMessage,
  type StoredFlight,
} from "../embed/bridge";
import type { MultiSceneCallbacks, MultiSceneHandle } from "./multiScene";
import { attachGestureFollowBreaker } from "./touchGestures";

/**
 * Lightweight WebGL-free MultiScene stub used ONLY when `VITE_E2E_SKIP_GLOBE=1`,
 * preserving bridge/flight/follow/gesture state semantics without importing Cesium.
 */
export function createE2eMultiSceneStub(
  container: HTMLElement,
  callbacks: MultiSceneCallbacks = {},
): MultiSceneHandle {
  let flights = new Map<string, StoredFlight>();
  let following = false;
  let paused = false;

  const detachGestures = attachGestureFollowBreaker(
    container,
    () => following,
    () => {
      if (!following) return;
      following = false;
      callbacks.onFollowChanged?.({ on: false, reason: "gesture" });
    },
  );

  callbacks.onPainted?.();

  return {
    applyFlights: (msg: FlightsMessage) => {
      flights = applyFlightsUpdate(flights, msg).flights;
    },
    setOwnFlightId: () => {},
    setUnits: () => {},
    refreshLocale: () => {},
    setFollow: (on: boolean) => {
      following = on;
    },
    recenter: () => {},
    overview: () => {
      following = false;
    },
    setPaused: (nextPaused: boolean) => {
      paused = nextPaused;
      void paused;
    },
    isFollowing: () => following,
    isPainted: () => true,
    flightCount: () => flights.size,
    pointCount: (id: string) => flights.get(id)?.points.length ?? 0,
    getQualityLevel: () => 0,
    resize: () => {},
    destroy: () => {
      detachGestures();
    },
  };
}
