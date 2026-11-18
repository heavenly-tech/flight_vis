-- Disk library ownership: public API slug may differ from on-disk folder (e.g. demo -> vuelo-5).
-- Source flight directories under {FLIGHT_DATA}/flights/ are never deleted by migration.
-- owner_id is the library principal (pilots.id today; global accounts.id when that model lands).

CREATE TABLE IF NOT EXISTS disk_flight_owners (
    public_slug TEXT PRIMARY KEY NOT NULL,
    storage_slug TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id) ON DELETE CASCADE,
    migrated_at BIGINT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_disk_flight_owners_storage
    ON disk_flight_owners (storage_slug);

CREATE INDEX IF NOT EXISTS idx_disk_flight_owners_owner
    ON disk_flight_owners (owner_id, club_id);

CREATE TABLE IF NOT EXISTS disk_live_session_owners (
    session_id TEXT PRIMARY KEY NOT NULL,
    owner_id TEXT NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id) ON DELETE CASCADE,
    device_label TEXT NOT NULL DEFAULT '',
    fix_count INTEGER NOT NULL DEFAULT 0,
    log_bytes BIGINT NOT NULL DEFAULT 0,
    migrated_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_disk_live_session_owners_owner
    ON disk_live_session_owners (owner_id, club_id);