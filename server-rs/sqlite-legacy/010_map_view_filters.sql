ALTER TABLE clubs ADD COLUMN latitude REAL;
ALTER TABLE clubs ADD COLUMN longitude REAL;
ALTER TABLE clubs ADD COLUMN public_radius_km REAL NOT NULL DEFAULT 100;
ALTER TABLE clubs ADD COLUMN show_public_tracks INTEGER NOT NULL DEFAULT 1;
ALTER TABLE clubs ADD COLUMN public_flights_cap INTEGER NOT NULL DEFAULT 50;
ALTER TABLE clubs ADD COLUMN public_altitude_mode TEXT NOT NULL DEFAULT 'agl';
ALTER TABLE clubs ADD COLUMN public_altitude_min_ft REAL NOT NULL DEFAULT 300;

ALTER TABLE pilots ADD COLUMN public_radius_km REAL NOT NULL DEFAULT 100;
ALTER TABLE pilots ADD COLUMN show_public_tracks INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pilots ADD COLUMN public_flights_cap INTEGER NOT NULL DEFAULT 50;
ALTER TABLE pilots ADD COLUMN public_altitude_mode TEXT NOT NULL DEFAULT 'agl';
ALTER TABLE pilots ADD COLUMN public_altitude_min_ft REAL NOT NULL DEFAULT 300;
ALTER TABLE pilots ADD COLUMN last_latitude REAL;
ALTER TABLE pilots ADD COLUMN last_longitude REAL;
ALTER TABLE pilots ADD COLUMN last_position_at INTEGER;

UPDATE clubs SET latitude = -33.4489, longitude = -70.6693 WHERE latitude IS NULL;

CREATE TABLE IF NOT EXISTS email_suppressions (
    email TEXT PRIMARY KEY NOT NULL,
    reason TEXT NOT NULL,
    created_at INTEGER NOT NULL
);