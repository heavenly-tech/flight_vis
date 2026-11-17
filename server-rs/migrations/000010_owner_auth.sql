-- Owner auth: login/magic throttle audit tables and guest claim flow.

CREATE TABLE IF NOT EXISTS login_attempts (
    ip TEXT NOT NULL,
    email TEXT NOT NULL DEFAULT '',
    ts BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_login_attempts_ip_ts ON login_attempts (ip, ts DESC);

CREATE TABLE IF NOT EXISTS magic_send_log (
    email TEXT NOT NULL,
    ip TEXT NOT NULL,
    ts BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_magic_send_email_ts ON magic_send_log (email, ts DESC);
CREATE INDEX IF NOT EXISTS idx_magic_send_ip_ts ON magic_send_log (ip, ts DESC);

CREATE TABLE IF NOT EXISTS guest_sessions (
    id TEXT PRIMARY KEY,
    club_id TEXT NOT NULL REFERENCES clubs(id) ON DELETE CASCADE,
    guest_pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    claimed_at BIGINT
);

CREATE INDEX IF NOT EXISTS idx_guest_sessions_pilot ON guest_sessions (guest_pilot_id);