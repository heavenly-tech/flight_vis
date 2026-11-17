CREATE TABLE IF NOT EXISTS flight_shares (
    id TEXT PRIMARY KEY NOT NULL,
    flight_id TEXT NOT NULL REFERENCES flights(id) ON DELETE CASCADE,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    created_by TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at BIGINT NOT NULL,
    expires_at BIGINT,
    revoked_at BIGINT
);

CREATE INDEX IF NOT EXISTS idx_flight_shares_flight ON flight_shares (flight_id);
CREATE INDEX IF NOT EXISTS idx_flight_shares_token_hash ON flight_shares (token_hash);