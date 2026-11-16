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
});