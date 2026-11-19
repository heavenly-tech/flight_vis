CREATE TABLE IF NOT EXISTS access_requests (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    requested_tail TEXT NOT NULL,
    type_id TEXT REFERENCES aircraft_types(id),
    new_type_name TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    reject_reason TEXT,
    created_at INTEGER NOT NULL,
    decided_at INTEGER,
    decided_by_pilot_id TEXT REFERENCES pilots(id)
);

CREATE TABLE IF NOT EXISTS audit_log (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    actor_pilot_id TEXT REFERENCES pilots(id),
    action TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    detail TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_access_requests_club_status
    ON access_requests (club_id, status);
CREATE INDEX IF NOT EXISTS idx_access_requests_pilot
    ON access_requests (pilot_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_log_club_time
    ON audit_log (club_id, created_at DESC);