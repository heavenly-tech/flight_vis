ALTER TABLE clubs
    ADD COLUMN trail_minutes_default SMALLINT NOT NULL DEFAULT 5
        CONSTRAINT clubs_trail_minutes_default_check
        CHECK (trail_minutes_default IN (-1, 0, 2, 5, 10, 15, 30));

ALTER TABLE flights
    ADD COLUMN trail_minutes SMALLINT NULL
        CONSTRAINT flights_trail_minutes_check
        CHECK (trail_minutes IS NULL OR trail_minutes IN (-1, 0, 2, 5, 10, 15, 30));
