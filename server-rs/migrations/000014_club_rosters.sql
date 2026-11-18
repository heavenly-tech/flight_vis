-- Club admin rosters: audit detail columns and CSV import staging (additive only).

ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS before_json TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS after_json TEXT;
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS bulk_batch_id TEXT;

CREATE INDEX IF NOT EXISTS idx_audit_log_bulk_batch ON audit_log (bulk_batch_id)
    WHERE bulk_batch_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS roster_import_batches (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id) ON DELETE CASCADE,
    actor_pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    preview_json TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_roster_import_batches_expires ON roster_import_batches (expires_at);