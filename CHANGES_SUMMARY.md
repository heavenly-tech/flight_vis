# Feature: Kiosk Globe Screen Always Shows a Map

## Summary
Implemented a 2D tile map display on the kiosk globe screen that always shows a map centered on the club's base aerodrome, even when there are no active flights.

## Backend Changes (Rust)

### 1. Migration
- **File**: `server-rs/migrations/000026_club_base_aerodrome_cas.sql`
- Sets `cas` club base to Tobalaba (SCTB: -33.456, -70.548)
- Sets demo club (admin@demo.test) base to SCTB if it exists

### 2. Database Functions
- **File**: `server-rs/src/club/db_kiosk_pg.rs`
- Added `validate_base_aerodrome(icao, lat, lon)` pure validation function
- Added `ClubBaseAerodrome` struct with Serialize derive
- Added `club_base_aerodrome(&self, club_id)` to get club's base
- Added `set_club_base_aerodrome(&self, club_id, icao, lat, lon)` to set club's base
- Added comprehensive unit tests for validation function

### 3. API Routes
- **File**: `server-rs/src/club/routes.rs`
- Added `GET /api/display/base` - returns `{icao, lat, lon}` for display's club
- Added `GET /api/admin/club/base` - returns current base
- Added `PATCH /api/admin/club/base` - updates base with validation
- All routes use existing auth patterns (`require_display_club`, `require_club_admin_scope`)
- Returns 400 with specific error messages for invalid ICAO/lat/lon

### 4. Demo Seeding
- **File**: `server-rs/src/club/db.rs`
- Updated `seed_demo_if_empty()` to ensure demo club has base aerodrome
- Added import for `ClubBaseAerodrome`

## Frontend Changes (TypeScript)

### 5. Kiosk Map View Module
- **File**: `src/club/kioskMapView.ts`
- Constants: `DEFAULT_BASE_VIEW_KM = 15`, fallback base (Santiago), `FALLBACK_VIEW_KM = 60`
- `computeKioskView(base, aircraft, viewportPx)` - computes center and zoom
- Web-Mercator math: `lonToTileX`, `latToTileY`, `metersPerPixel`
- Helper functions: `getFallbackBase()`, `getFallbackViewKm()`

### 6. Kiosk Map View Tests
- **File**: `src/club/kioskMapView.test.ts`
- Tests for no flights with base → base center
- Tests for no base and no flights → fallback
- Tests for one/several flights → bbox contains all
- Tests for zoom clamping [3, 15]
- Tests for Web-Mercator math functions
- Tests for returning to base view when flights go to zero

### 7. Club Display Core
- **File**: `src/club/clubDisplayCore.ts`
- Added tile map rendering with ArcGIS World Imagery tiles
- Added aircraft markers with tail labels
- Added base aerodrome marker with ICAO code
- Added Esri attribution in corner
- Map container has `data-testid="kiosk-map"` and `data-view` attribute
- Recomputes view on each `refreshMap()` (every 5s) and on resize
- Fetches base once after activation and reuses it

### 8. HTML Updates
- **File**: `club-globe-screen.html`
- Added CSS for `#kiosk-map` container

### 9. Admin UI
- **File**: `club-admin-map.html`
- Added "Aeródromo base" section with:
  - ICAO input (maxlength=4, uppercase)
  - Latitude input (number, step=0.0001, min=-90, max=90)
  - Longitude input (number, step=0.0001, min=-180, max=180)
  - Save button
  - Message line

- **File**: `src/club/adminMap.ts`
- Added element selectors for base aerodrome inputs
- Added load functionality in initialization
- Added save handler with validation error display
- Follows exact pattern of club locale and slug settings

### 10. Locale Strings
- **Files**: `locales/es.json`, `locales/en.json`
- Added to `admin` section:
  - `aerodromeBase`: "Aeródromo base" / "Base aerodrome"
  - `icaoCode`: "Código OACI (4 letras)" / "ICAO code (4 letters)"
  - `latitude`: "Latitud" / "Latitude"
  - `longitude`: "Longitud" / "Longitude"
  - `saveBase`: "Guardar aeródromo" / "Save aerodrome"
  - `baseSaved`: "Aeródromo guardado." / "Aerodrome saved."
  - `invalidBase`: "Revisa el código OACI y las coordenadas." / "Check the ICAO code and coordinates."

## Tests

### 11. e2e Tests
- **File**: `e2e/kiosk-base-map.spec.ts`
- Test: kiosk globe screen shows map with base view when no aircraft
  - Logs in as admin
  - Creates display session
  - Stubs ArcGIS tiles
  - Stubs `/api/display/map` to return empty aircraft
  - Activates display
  - Asserts `[data-testid="kiosk-map"]` is visible
  - Asserts at least one tile `img` is present
  - Asserts `data-view` is "base" or "fallback"

- Test: admin can set and get club base aerodrome
  - PATCH `/api/admin/club/base` with valid SCTB data → 200
  - GET returns the saved values

- Test: admin cannot set invalid base aerodrome
  - Invalid ICAO "SC" → 400 "invalid icao"
  - Lat 95 → 400 "invalid lat"
  - Lon -181 → 400 "invalid lon"

- Test: display base endpoint returns club base
  - Creates display session
  - Activates it
  - GET `/api/display/base` returns base data

## Verification

All changes:
- Follow existing code patterns and style
- Use proper validation (ICAO via `validate_icao`, lat/lon in range)
- Maintain Spanish (es-CL) with tú forms, never voseo
- Are clippy-clean and rustfmt-styled
- Pass `npm run lint` and `npm run test:unit`
- Do not add new HTML pages (no CSP table updates needed)
- ArcGIS tiles already allowed by existing Globe CSP
