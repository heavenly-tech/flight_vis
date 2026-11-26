import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const stylePath = join(dirname(fileURLToPath(import.meta.url)), "../style.css");
const style = readFileSync(stylePath, "utf8");

describe("HUD layout CSS", () => {
  it("scopes portrait DIST/UTC grid spans away from landscape", () => {
    expect(style).toMatch(/@media \(orientation: portrait\) \{[\s\S]*grid-column: 1 \/ span 2/);
    expect(style).toMatch(/@media \(orientation: portrait\) \{[\s\S]*grid-column: 3 \/ span 2/);
    expect(style).toMatch(
      /@media \(max-width: 1023px\) and \(orientation: landscape\) \{[\s\S]*grid-row: auto;/,
    );
    expect(style).toMatch(
      /@media \(max-width: 1023px\) and \(orientation: landscape\) \{[\s\S]*grid-column: auto;/,
    );
    expect(style).toMatch(
      /@media \(max-width: 1023px\) and \(orientation: landscape\) \{[\s\S]*body\.hud-strip-expanded \.hud \{[\s\S]*max-height: none;/,
    );
  });

  it("locks full-viewport globe layout, safe-area edge anchoring, and left library position", () => {
    expect(style).toMatch(
      /#cesiumContainer \{[\s\S]*?position: fixed;[\s\S]*?inset: 0;[\s\S]*?width: 100vw;[\s\S]*?height: 100dvh;/,
    );
    expect(style).toMatch(/body\.fv-page #cesiumContainer \{[\s\S]*?position: relative;/);
    expect(style).toMatch(
      /#ui \{[\s\S]*?position: fixed;[\s\S]*?inset: 0;[\s\S]*?width: 100vw;[\s\S]*?height: 100dvh;[\s\S]*?pointer-events: none;[\s\S]*?padding-top: max\(8px, env\(safe-area-inset-top\)\);[\s\S]*?padding-right: max\(8px, env\(safe-area-inset-right\)\);[\s\S]*?padding-bottom: max\(8px, env\(safe-area-inset-bottom\)\);[\s\S]*?padding-left: max\(8px, env\(safe-area-inset-left\)\);/,
    );
    expect(style).toMatch(
      /@media \(min-width: 1024px\) \{[\s\S]*?\.hud \{[\s\S]*?right: max\(8px, env\(safe-area-inset-right\)\);/,
    );
    expect(style).toMatch(/\.library-panel \{[\s\S]*?grid-area: library;/);
    expect(style).toMatch(
      /@media \(max-width: 1023px\) and \(orientation: landscape\) \{[\s\S]*?body\.telemetry-hidden #ui \{/,
    );
  });
});
