-- Self-serve club signup: optional aerodrome ICAO, club-scoped magic tokens (additive only).

ALTER TABLE clubs ADD COLUMN IF NOT EXISTS aerodrome_icao TEXT;

ALTER TABLE magic_login_tokens
    ADD COLUMN IF NOT EXISTS club_id TEXT REFERENCES clubs(id) ON DELETE CASCADE;