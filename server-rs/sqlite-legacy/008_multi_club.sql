ALTER TABLE clubs ADD COLUMN slug TEXT;

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
    created_at INTEGER NOT NULL,
    UNIQUE (pilot_id, club_id)
);

INSERT OR IGNORE INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
SELECT
    lower(hex(randomblob(16))),
    p.id,
    p.club_id,
    p.role,
    p.status,
    p.created_at
FROM pilots p;

ALTER TABLE pilots ADD COLUMN global_role TEXT NOT NULL DEFAULT '';

ALTER TABLE flights ADD COLUMN club_id TEXT REFERENCES clubs(id);

UPDATE flights
SET club_id = (
    SELECT a.club_id FROM aircraft a WHERE a.id = flights.aircraft_id
)
WHERE club_id IS NULL;

ALTER TABLE checkouts ADD COLUMN club_id TEXT REFERENCES clubs(id);

UPDATE checkouts
SET club_id = (
    SELECT p.club_id FROM pilots p WHERE p.id = checkouts.pilot_id
)
WHERE club_id IS NULL;