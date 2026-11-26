import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

const DATABASE_URL =
  process.env.DATABASE_URL ?? "postgres://postgres:postgres@127.0.0.1:5432/flight_vis_test";

const OTHER_CLUB_LABEL_RE = /^(Fuera del club|Other club)$/;

function runPsql(sql: string): void {
  execFileSync("psql", [DATABASE_URL, "-v", "ON_ERROR_STOP=1", "-c", sql], {
    stdio: "pipe",
  });
}

type SeededNearbyFlight = {
  clubId: string;
  pilotId: string;
  deviceId: string;
  typeId: string;
  aircraftId: string;
  flightId: string;
  tail: string;
};

/**
 * Seeds an active, verified second club and a live `public` flight with a curved 12-minute
 * trail ~5 km northeast of the demo club's SCTB base (-33.456, -70.548).
 */
function seedNearbyOtherClubFlight(): SeededNearbyFlight {
  const tag = crypto.randomBytes(4).toString("hex");
  const clubId = `e2e-np-club-${tag}`;
  const slug = `e2e-nearby-${tag}`;
  const pilotId = `e2e-np-pilot-${tag}`;
  const deviceId = `e2e-np-dev-${tag}`;
  const typeId = `e2e-np-type-${tag}`;
  const aircraftId = `e2e-np-ac-${tag}`;
  const flightId = `e2e-np-flt-${tag}`;
  const tail = `CC-EXT${tag.slice(0, 2).toUpperCase()}`;
  const now = Date.now();
  const startedAt = now - 14 * 60_000;

  const fixRows: string[] = [];
  for (let i = 0; i <= 12; i++) {
    const frac = i / 12;
    const arc = Math.sin(frac * Math.PI);
    const tMs = startedAt + i * 60_000;
    // Curved track ~6-9 km NW of SCTB (-33.456, -70.548), clear of N101 (NE) and N202 (SW).
    const lat = (-33.438 + frac * 0.038 + arc * 0.008).toFixed(6);
    const lon = (-70.585 - frac * 0.036 + arc * 0.007).toFixed(6);
    const alt = (1080 + i * 14).toFixed(1);
    fixRows.push(
      `('${flightId}', '${deviceId}', ${tMs}, ${tMs}, ${lat}, ${lon}, ${alt}, 'wgs84_ellipsoid', 34.0, 315.0, 5.0)`,
    );
  }
  const latestT = startedAt + 12 * 60_000;

  runPsql(`
    BEGIN;
    INSERT INTO clubs (id, name, slug, created_at, status, contact_email, email_verified_at,
                       checkin_window_hours, aerodrome_icao, latitude, longitude,
                       trail_minutes_default, show_nearby_public, nearby_public_radius_km)
    VALUES ('${clubId}', 'Club Vecino E2E', '${slug}', ${now}, 'live', 'vecino-${tag}@e2e.test', ${now},
            12, 'SCEL', -33.393, -70.786, 15, TRUE, 50);

    INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
    VALUES ('${pilotId}', '${clubId}', 'Piloto Externo', 'ext-${tag}@e2e.test', 'EXT-1', 'approved', 'pilot', '', ${now}, ${now});

    INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
    VALUES ('e2e-np-mem-${tag}', '${pilotId}', '${clubId}', 'pilot', 'active', ${now});

    INSERT INTO aircraft_types (id, club_id, code)
    VALUES ('${typeId}', '${clubId}', 'C172');

    INSERT INTO aircraft (id, club_id, tail, type_id, active)
    VALUES ('${aircraftId}', '${clubId}', '${tail}', '${typeId}', TRUE);

    INSERT INTO devices (id, pilot_id, token_hash, label, created_at)
    VALUES ('${deviceId}', '${pilotId}', 'e2e-np-hash-${tag}', 'E2E Nearby', ${now});

    INSERT INTO flights (id, pilot_id, aircraft_id, club_id, status, started_at, last_fix_at,
                         visibility, show_pilot_name_public, trail_minutes)
    VALUES ('${flightId}', '${pilotId}', '${aircraftId}', '${clubId}', 'active', ${startedAt}, ${latestT},
            'public', FALSE, NULL);

    INSERT INTO flight_devices (flight_id, device_id)
    VALUES ('${flightId}', '${deviceId}');

    INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, alt_ref, speed, course, accuracy)
    VALUES ${fixRows.join(",\n           ")};
    COMMIT;
  `);

  return { clubId, pilotId, deviceId, typeId, aircraftId, flightId, tail };
}

function cleanupNearbyOtherClubFlight(seed: SeededNearbyFlight): void {
  runPsql(`
    BEGIN;
    DELETE FROM fixes WHERE flight_id = '${seed.flightId}';
    DELETE FROM flight_devices WHERE flight_id = '${seed.flightId}';
    DELETE FROM flights WHERE id = '${seed.flightId}';
    DELETE FROM devices WHERE id = '${seed.deviceId}';
    DELETE FROM aircraft WHERE id = '${seed.aircraftId}';
    DELETE FROM aircraft_types WHERE id = '${seed.typeId}';
    DELETE FROM club_memberships WHERE club_id = '${seed.clubId}';
    DELETE FROM pilots WHERE id = '${seed.pilotId}';
    DELETE FROM clubs WHERE id = '${seed.clubId}';
    COMMIT;
  `);
}

test.describe("nearby public flights from other clubs on admin map and club screen", () => {
  test.setTimeout(60_000);

  test("shows nearby non-club flight with distinct marker/label/trail on admin map and club screen, and syncs admin toggle off/on", async ({
    context,
    page,
  }) => {
    const seeded = seedNearbyOtherClubFlight();

    try {
      const adminLogin = await page.request.post("/api/club/login", {
        data: { email: "admin@demo.test", password: "password12" },
      });
      expect(adminLogin.ok()).toBeTruthy();

      // Ensure demo club starts with nearby public enabled (50 km) and all own tails visible.
      const clearTails = await page.request.post("/api/admin/map/tails", {
        data: { tails: [] },
      });
      expect(clearTails.ok()).toBeTruthy();

      const resetNearby = await page.request.patch("/api/admin/club/nearby-public", {
        data: { showNearbyPublic: true, nearbyPublicRadiusKm: 50 },
      });
      expect(resetNearby.ok()).toBeTruthy();

      // Open admin live map.
      await stubArcgisTiles(page);
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });

      const adminMap = page.locator('[data-testid="admin-map"]');
      const adminNonClubMarker = adminMap.locator(
        `[data-marker="aircraft"][data-tail="${seeded.tail}"][data-non-club="true"]`,
      );
      const adminNonClubTrail = adminMap.locator(
        `[data-testid="live-trail"][data-trail-tail="${seeded.tail}"][data-non-club="true"]`,
      );

      await expect(adminNonClubMarker).toBeVisible({ timeout: 15_000 });
      await expect(adminNonClubMarker.locator('[data-non-club-label="true"]')).toHaveText(
        OTHER_CLUB_LABEL_RE,
      );
      await expect(adminNonClubTrail).toHaveCount(1, { timeout: 15_000 });
      await expect(adminNonClubTrail).toHaveAttribute("data-trail-minutes", "15");

      // Open club-screen.html in a second page of the same browser context.
      const screenPage = await context.newPage();
      await stubArcgisTiles(screenPage);
      await screenPage.setViewportSize({ width: 1920, height: 1080 });
      await screenPage.goto("/club-screen.html", { waitUntil: "networkidle" });

      const kioskMap = screenPage.locator('[data-testid="kiosk-map"]');
      const screenNonClubMarker = kioskMap.locator(
        `[data-marker="aircraft"][data-tail="${seeded.tail}"][data-non-club="true"]`,
      );
      const screenNonClubTrail = kioskMap.locator(
        `[data-testid="live-trail"][data-trail-tail="${seeded.tail}"][data-non-club="true"]`,
      );
      const screenNonClubChip = screenPage.locator(
        `#map button.kiosk-tail-chip[data-tail="${seeded.tail}"][data-non-club="true"]`,
      );

      await expect(screenNonClubMarker).toBeVisible({ timeout: 15_000 });
      await expect(screenNonClubMarker.locator('[data-non-club-label="true"]')).toHaveText(
        OTHER_CLUB_LABEL_RE,
      );
      await expect(screenNonClubChip).toBeVisible();
      await expect(screenNonClubTrail).toHaveCount(1, { timeout: 15_000 });
      await expect(screenNonClubTrail).toHaveAttribute("data-trail-minutes", "15");

      // Open club settings on admin map.
      await page.locator("#club-settings > summary").click();
      await expect(page.locator("#club-settings")).toHaveAttribute("open", "");
      const toggle = page.locator('[data-testid="club-nearby-public-toggle"]');
      const radiusSelect = page.locator('[data-testid="club-nearby-public-radius"]');
      await expect(toggle).toBeChecked();
      await expect(radiusSelect).toHaveValue("50");

      const shotsDir = process.env.SHOTS_DIR;
      if (shotsDir) {
        fs.mkdirSync(shotsDir, { recursive: true });
        await screenPage.screenshot({
          path: path.join(shotsDir, "club-screen-nearby-1920x1080.png"),
          fullPage: false,
        });
        await page.locator("#club-nearby-public").scrollIntoViewIfNeeded();
        await page.screenshot({
          path: path.join(shotsDir, "admin-settings-nearby-1280x800.png"),
          fullPage: false,
        });
      }

      // Admin toggles nearby public OFF -> both admin map and club screen remove it via sync!
      await toggle.uncheck();
      await expect(toggle).not.toBeChecked({ timeout: 10_000 });
      await expect(radiusSelect).toBeDisabled();
      await expect(page.locator("#club-nearby-public-msg")).toHaveAttribute("role", "status", {
        timeout: 10_000,
      });

      await expect(adminNonClubMarker).toHaveCount(0, { timeout: 10_000 });
      await expect(adminNonClubTrail).toHaveCount(0, { timeout: 10_000 });
      await expect(screenNonClubMarker).toHaveCount(0, { timeout: 10_000 });
      await expect(screenNonClubTrail).toHaveCount(0, { timeout: 10_000 });
      await expect(screenNonClubChip).toHaveCount(0, { timeout: 10_000 });

      // Toggle back ON -> reappears on both admin map and club screen!
      await toggle.check();
      await expect(toggle).toBeChecked({ timeout: 10_000 });
      await expect(radiusSelect).toBeEnabled();

      await expect(adminNonClubMarker).toBeVisible({ timeout: 10_000 });
      await expect(adminNonClubTrail).toHaveCount(1, { timeout: 10_000 });
      await expect(screenNonClubMarker).toBeVisible({ timeout: 10_000 });
      await expect(screenNonClubTrail).toHaveCount(1, { timeout: 10_000 });

      await screenPage.close();
    } finally {
      await page.request
        .patch("/api/admin/club/nearby-public", {
          data: { showNearbyPublic: true, nearbyPublicRadiusKm: 50 },
        })
        .catch(() => {});
      cleanupNearbyOtherClubFlight(seeded);
    }
  });
});
