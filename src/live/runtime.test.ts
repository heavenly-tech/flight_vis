import { describe, expect, it } from "vitest";
import { liveStatusAfterStreamOpen } from "./liveGeoidStatus";
import { msg } from "./messages";
import { parseLivePath } from "./runtime";

describe("live boot routing", () => {
  it("keeps geoid pending notice on SSE open until the grid is ready", () => {
    const pending = msg("geoidPending", "en");
    expect(liveStatusAfterStreamOpen(false, pending)).toBe(pending);
    expect(liveStatusAfterStreamOpen(true, pending)).toBeNull();
  });

  it("parses /live/:id", () => {
    expect(parseLivePath("/live/sess-abc")).toEqual({ listOnly: false, sessionId: "sess-abc" });
    expect(parseLivePath("/live")).toEqual({ listOnly: true, sessionId: null });
    expect(parseLivePath("/vuelo-5")).toEqual({ listOnly: false, sessionId: null });
  });
});