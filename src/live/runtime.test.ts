import { describe, expect, it } from "vitest";
import { parseLivePath } from "./runtime";

describe("live boot routing", () => {
  it("parses /live/:id", () => {
    expect(parseLivePath("/live/sess-abc")).toEqual({ listOnly: false, sessionId: "sess-abc" });
    expect(parseLivePath("/live")).toEqual({ listOnly: true, sessionId: null });
    expect(parseLivePath("/vuelo-5")).toEqual({ listOnly: false, sessionId: null });
  });
});