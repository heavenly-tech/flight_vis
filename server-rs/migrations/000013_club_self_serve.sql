-- Self-serve club signup: optional aerodrome ICAO, club-scoped magic tokens (additive only).

ALTER TABLE clubs ADD COLUMN IF NOT EXISTS aerodrome_icao TEXT;

ALTER TABLE magic_login_tokens
    ADD COLUMN IF NOT EXISTS club_id TEXT REFERENCES clubs(id) ON DELETE CASCADE;

-- Legacy clubs were `live` before email verification was enforced.
UPDATE clubs
SET email_verified_at = created_at
WHERE status = 'live' AND email_verified_at IS NULL;