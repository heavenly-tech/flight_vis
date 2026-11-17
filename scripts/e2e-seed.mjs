#!/usr/bin/env node
/**
 * Prepare CI Postgres for Playwright: ensure database exists (service image already has flight_vis_test).
 * Demo pilots/club are seeded by the Rust API on boot (FLIGHT_CLUB_SLUG=demo, seed_demo_if_empty).
 */
const url =
  process.env.DATABASE_URL ??
  "postgres://postgres:postgres@127.0.0.1:5432/flight_vis_test";

console.log(`e2e-seed: using ${url.replace(/:[^:@/]+@/, ":***@")}`);
console.log("e2e-seed: demo data is created when the API starts (seed_demo_if_empty).");