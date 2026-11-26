-- Platform-level tombstones for hard-deleted clubs (M-21) and slug reuse cooldown (M-20).
-- No foreign key to clubs: the club row is deleted in the same transaction.
-- Migration 000029 is reserved by feat/live-trail.

CREATE TABLE IF NOT EXISTS club_delete_tombstones (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL,
    club_name TEXT NOT NULL,
    actor_account_id TEXT REFERENCES accounts (id) ON DELETE SET NULL,
    deleted_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_club_delete_tombstones_deleted_at
    ON club_delete_tombstones (deleted_at DESC);

CREATE TABLE IF NOT EXISTS club_delete_tombstone_slugs (
    tombstone_id TEXT NOT NULL REFERENCES club_delete_tombstones (id) ON DELETE CASCADE,
    slug TEXT NOT NULL,
    PRIMARY KEY (tombstone_id, slug)
);

CREATE INDEX IF NOT EXISTS idx_club_delete_tombstone_slugs_slug
    ON club_delete_tombstone_slugs (slug);
