import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { findDuplicateKeys } from "./duplicateKeys";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

describe("findDuplicateKeys", () => {
  it("finds top-level duplicate keys", () => {
    const text = '{"a":{"x":1},"b":2,"a":{"y":2}}';
    expect(findDuplicateKeys(text)).toEqual(["a"]);
  });

  it("finds nested duplicate keys", () => {
    const text = '{"a":{"x":1,"x":2}}';
    expect(findDuplicateKeys(text)).toEqual(["a.x"]);
  });

  it("ignores keys inside strings", () => {
    const text = '{"a":{"x":1},"b":"{\"a\":2}","a":{"y":2}}';
    expect(findDuplicateKeys(text)).toEqual(["a"]);
  });

  it("ignores escaped quotes in strings", () => {
    const text = '{"a":{"x":1},"b":"key\\\"value"},"a":{"y":2}}';
    expect(findDuplicateKeys(text)).toEqual(["a"]);
  });

  it("ignores array elements", () => {
    const text = '{"a":[{"x":1},{"x":2}]}';
    expect(findDuplicateKeys(text)).toEqual([]);
  });

  it("handles deeply nested duplicates", () => {
    const text = '{"a":{"b":{"c":1,"c":2}}}';
    expect(findDuplicateKeys(text)).toEqual(["a.b.c"]);
  });

  it("handles multiple duplicates", () => {
    const text = '{"a":1,"b":2,"a":3,"c":{"d":4,"d":5}}';
    const result = findDuplicateKeys(text);
    expect(result).toContain("a");
    expect(result).toContain("c.d");
  });

  it("handles empty objects", () => {
    const text = '{"a":{},"b":{}}';
    expect(findDuplicateKeys(text)).toEqual([]);
  });

  it("handles whitespace variations", () => {
    const text = '{ "a" : { "x" : 1 } , "a" : { "y" : 2 } }';
    expect(findDuplicateKeys(text)).toEqual(["a"]);
  });
});

describe("locale files have no duplicate keys", () => {
  it("all locale JSON files have no duplicate keys", () => {
    const localesDir = path.join(root, "locales");
    const files = fs.readdirSync(localesDir).filter((f) => f.endsWith(".json"));

    const violations: string[] = [];
    for (const file of files) {
      const filePath = path.join(localesDir, file);
      const text = fs.readFileSync(filePath, "utf8");
      const duplicates = findDuplicateKeys(text);
      for (const dup of duplicates) {
        violations.push(`${file}: duplicate key "${dup}"`);
      }
    }

    expect(violations).toEqual([]);
  });
});
