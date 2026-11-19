#!/usr/bin/env node
/**
 * Regenerate PNG/ICO web icons from assets/brand SVGs (offline; uses @resvg/resvg-js).
 * Run: npm run export:icons
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";
import toIco from "to-ico";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const brandDir = path.join(root, "assets", "brand");
const publicDir = path.join(root, "public");
const iconsDir = path.join(publicDir, "icons");

const sources = {
  full: path.join(brandDir, "flightvis-logo.svg"),
  small: path.join(brandDir, "flightvis-logo-small.svg"),
  maskable: path.join(brandDir, "flightvis-logo-maskable.svg"),
};

function renderPng(svgPath, size) {
  const svg = fs.readFileSync(svgPath, "utf8");
  const resvg = new Resvg(svg, {
    fitTo: { mode: "width", value: size },
    background: "transparent",
  });
  return resvg.render().asPng();
}

function writePng(outPath, data) {
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, data);
}

function copyFile(src, dest) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
}

async function main() {
  const pngJobs = [
    { src: sources.small, out: path.join(publicDir, "favicon.svg"), copy: true },
    { src: sources.small, size: 16, out: path.join(publicDir, "favicon-16.png") },
    { src: sources.small, size: 32, out: path.join(publicDir, "favicon-32.png") },
    { src: sources.small, size: 48, out: path.join(publicDir, "favicon-48.png") },
    { src: sources.small, size: 180, out: path.join(publicDir, "apple-touch-icon.png") },
    { src: sources.full, size: 192, out: path.join(iconsDir, "icon-192.png") },
    { src: sources.full, size: 512, out: path.join(iconsDir, "icon-512.png") },
    { src: sources.maskable, size: 512, out: path.join(iconsDir, "icon-512-maskable.png") },
  ];

  for (const job of pngJobs) {
    if (job.copy) {
      copyFile(job.src, job.out);
      continue;
    }
    writePng(job.out, renderPng(job.src, job.size));
    console.log(`wrote ${path.relative(root, job.out)}`);
  }

  const ico = await toIco([
    renderPng(sources.small, 16),
    renderPng(sources.small, 32),
    renderPng(sources.small, 48),
  ]);
  writePng(path.join(publicDir, "favicon.ico"), ico);
  console.log("wrote public/favicon.ico");

  const brandPublic = path.join(publicDir, "assets", "brand");
  for (const name of [
    "flightvis-logo.svg",
    "flightvis-logo-small.svg",
    "flightvis-logo-maskable.svg",
  ]) {
    copyFile(path.join(brandDir, name), path.join(brandPublic, name));
  }
  console.log("synced public/assets/brand/");
}

await main();