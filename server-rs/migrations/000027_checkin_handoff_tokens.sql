-- Check-in handoff tokens for browser-to-app handoff
-- Single-use, short-lived (~10 min), club-scoped tokens that survive QR rotation

CREATE TABLE IF NOT EXISTS checkin_handoff_tokens (
    id TEXT PRIMARY KEY NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    club_id TEXT NOT NULL,
    pilot_id TEXT NOT NULL,
    device_id TEXT,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    consumed_at BIGINT,
    consumed_by_device_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_checkin_handoff_tokens_hash ON checkin_handoff_tokens (token_hash);
CREATE INDEX IF NOT EXISTS idx_checkin_handoff_tokens_pilot ON checkin_handoff_tokens (pilot_id);
CREATE INDEX IF NOT EXISTS idx_checkin_handoff_tokens_club ON checkin_handoff_tokens (club_id);
CREATE INDEX IF NOT EXISTS idx_checkin_handoff_tokens_expires ON checkin_handoff_tokens (expires_at) WHERE consumed_at IS NULL;
