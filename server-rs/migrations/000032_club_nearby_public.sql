ALTER TABLE clubs
    ADD COLUMN show_nearby_public BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN nearby_public_radius_km SMALLINT NOT NULL DEFAULT 50
        CONSTRAINT clubs_nearby_public_radius_km_check
        CHECK (nearby_public_radius_km BETWEEN 5 AND 500);

CREATE INDEX IF NOT EXISTS idx_flights_active_last_fix
    ON flights (last_fix_at DESC)
    WHERE status = 'active';
