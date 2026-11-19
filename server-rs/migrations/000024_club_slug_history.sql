CREATE TABLE club_slug_history (
    old_slug TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs (id) ON DELETE CASCADE,
    changed_at BIGINT NOT NULL
);

CREATE INDEX idx_club_slug_history_club_time ON club_slug_history (club_id, changed_at DESC);