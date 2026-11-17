ALTER TABLE clubs ADD COLUMN IF NOT EXISTS slug TEXT;

UPDATE clubs
SET slug = COALESCE(
    NULLIF(trim(lower(replace(replace(name, ' ', '-'), '''', ''))), ''),
    'default'
)
WHERE slug IS NULL OR slug = '';

CREATE UNIQUE INDEX IF NOT EXISTS idx_clubs_slug ON clubs (slug);

CREATE TABLE IF NOT EXISTS club_memberships (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    club_id TEXT NOT NULL REFERENCES clubs(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE (pilot_id, club_id)
);

ALTER TABLE pilots ADD COLUMN IF NOT EXISTS global_role TEXT NOT NULL DEFAULT '';

ALTER TABLE flights ADD COLUMN IF NOT EXISTS club_id TEXT REFERENCES clubs(id);

ALTER TABLE checkouts ADD COLUMN IF NOT EXISTS club_id TEXT REFERENCES clubs(id);