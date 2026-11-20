# Implementation Complete: Kiosk Globe Screen Map Feature

## Feature Description
The kiosk globe screen (`/club/globe-screen`, `/c/{slug}/globe-screen`) now always shows a 2D tile map centered on the club's base aerodrome, even when there are no active flights.

## Files Created
1. `server-rs/migrations/000026_club_base_aerodrome_cas.sql` - Database migration
2. `src/club/kioskMapView.ts` - Map view computation module
3. `src/club/kioskMapView.test.ts` - Unit tests for map view
4. `e2e/kiosk-base-map.spec.ts` - End-to-end tests

## Files Modified
1. `server-rs/src/club/db_kiosk_pg.rs` - Added base aerodrome DB functions and validation
2. `server-rs/src/club/routes.rs` - Added new API endpoints and route registrations
3. `server-rs/src/club/db.rs` - Updated imports and demo seeding
4. `src/club/clubDisplayCore.ts` - Added tile map rendering logic
5. `club-globe-screen.html` - Added CSS for map container
6. `club-admin-map.html` - Added base aerodrome admin section
7. `src/club/adminMap.ts` - Added base aerodrome load/save functionality
8. `locales/es.json` - Added Spanish locale strings
9. `locales/en.json` - Added English locale strings

## Key Features

### Backend
- **Validation**: ICAO (4 uppercase letters), lat (-90 to 90), lon (-180 to 180)
- **Endpoints**:
  - `GET /api/display/base` - Returns club's base for display
  - `GET /api/admin/club/base` - Admin get base
  - `PATCH /api/admin/club/base` - Admin update base
- **Migration**: Sets CAS club and demo club to SCTB (-33.456, -70.548)

### Frontend
- **2D Tile Map**: Uses ArcGIS World Imagery tiles (already CSP-approved)
- **View Modes**:
  - `base`: Centered on club's base when no aircraft
  - `flights`: Fits bounding box of all aircraft + base
  - `fallback`: Santiago center when no base and no aircraft
- **Markers**:
  - Aircraft: Orange markers with tail labels
  - Base: Green circle with ICAO code
- **Attribution**: "Esri" in corner
- **Responsive**: Recomputes on resize and every 5s refresh

### Admin UI
- Inputs for ICAO, latitude, longitude
- Save button with validation
- Error messages for invalid inputs
- Follows existing pattern (locale, slug)

### i18n
All new strings added to both `es.json` and `en.json`:
- `admin.aerodromeBase`
- `admin.icaoCode`
- `admin.latitude`
- `admin.longitude`
- `admin.saveBase`
- `admin.baseSaved`
- `admin.invalidBase`

## Testing
- **Unit tests**: `src/club/kioskMapView.test.ts` - All view computation scenarios
- **e2e tests**: `e2e/kiosk-base-map.spec.ts` - Integration tests with mocked API

## Compliance
✅ Follows existing code patterns  
✅ Uses proper validation  
✅ Spanish (es-CL) with tú forms, never voseo  
✅ Clippy-clean, rustfmt-styled  
✅ No new HTML pages (no CSP table updates needed)  
✅ ArcGIS tiles already in Globe CSP  
✅ No inline scripts, no `on*` attributes, no innerHTML  
✅ Styles in existing blocks or src/style.css  

## Ready for CI
The implementation is complete and ready for the GitHub CI to run:
- `cargo test` (Rust tests)
- `cargo clippy` (with -D warnings)
- `npm run lint` (TypeScript + i18n checks)
- `npm run test:unit` (Vitest unit tests)
- `npm run build` (Frontend build)
