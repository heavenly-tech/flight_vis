#!/usr/bin/env node
import { globeE2eCsp } from "./read-html-csp.mjs";

const csp = globeE2eCsp();
const imgSrcMatches = csp.match(/\bimg-src\b/g) ?? [];
if (imgSrcMatches.length !== 1) {
  console.error(
    `globe CSP must contain exactly one img-src directive, found ${imgSrcMatches.length}: ${csp}`,
  );
  process.exit(1);
}