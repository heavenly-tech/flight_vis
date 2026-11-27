-- E1: Club-less (public pilot) mode — nullable pilots.club_id (personal pilot per account),
-- nullable flights.aircraft_id, free-text flights.tail_text, and scope/format CHECK constraints.

-- 1. Personal (club-less) pilot support on `pilots`.
ALTER TABLE pilots
    ALTER COLUMN club_id DROP NOT NULL;

ALTER TABLE pilots
    ADD CONSTRAINT pilots_personal_requires_account
    CHECK (club_id IS NOT NULL OR account_id IS NOT NULL) NOT VALID;

ALTER TABLE pilots
    VALIDATE CONSTRAINT pilots_personal_requires_account;

CREATE UNIQUE INDEX IF NOT EXISTS idx_pilots_personal_account
    ON pilots (account_id)
    WHERE club_id IS NULL;

-- 2. Club-less flight support on `flights`.
ALTER TABLE flights
    ALTER COLUMN aircraft_id DROP NOT NULL;

ALTER TABLE flights
    ADD COLUMN IF NOT EXISTS tail_text TEXT NULL;

-- Backfill legacy club flights created before 000006/club_id population so every
-- aircraft-backed row carries its aircraft's club_id before validating CHECKs.
UPDATE flights f
SET club_id = a.club_id
FROM aircraft a
WHERE f.club_id IS NULL
  AND f.aircraft_id = a.id;

ALTER TABLE flights
    ADD CONSTRAINT flights_tail_text_format_check
    CHECK (
        tail_text IS NULL
        OR (
            char_length(tail_text) BETWEEN 1 AND 16
            AND tail_text ~ '^[A-Z0-9-]+$'
            AND tail_text ~ '[A-Z0-9]'
        )
    ) NOT VALID;

ALTER TABLE flights
    VALIDATE CONSTRAINT flights_tail_text_format_check;

ALTER TABLE flights
    ADD CONSTRAINT flights_club_scope_check
    CHECK (
        (
            club_id IS NULL
            AND aircraft_id IS NULL
            AND tail_text IS NOT NULL
            AND visibility IN ('private', 'public')
        )
        OR (
            club_id IS NOT NULL
            AND aircraft_id IS NOT NULL
            AND tail_text IS NULL
        )
    ) NOT VALID;

ALTER TABLE flights
    VALIDATE CONSTRAINT flights_club_scope_check;
