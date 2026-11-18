ALTER TABLE clubs ADD COLUMN checkin_valid_minutes INTEGER NOT NULL DEFAULT 30;
ALTER TABLE clubs ADD COLUMN checkin_hmac_secret TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS display_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    created_by_pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    revoked_at INTEGER
);

CREATE TABLE IF NOT EXISTS checkins (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    pilot_id TEXT REFERENCES pilots(id),
    device_id TEXT REFERENCES devices(id),
    time_step INTEGER NOT NULL,
    token_digest TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    consumed_at INTEGER,
    outcome TEXT NOT NULL,
    fail_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_checkins_replay
    ON checkins (pilot_id, device_id, token_digest)
    WHERE outcome = 'ok';