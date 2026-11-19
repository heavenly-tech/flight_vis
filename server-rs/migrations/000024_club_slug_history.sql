CREATE TABLE club_slug_history (
    old_slug TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs (id) ON DELETE CASCADE,
    changed_at BIGINT NOT NULL
);

CREATE INDEX idx_club_slug_history_club_time ON club_slug_history (club_id, changed_at DESC);

-- Global club slug namespace (current + historical); enforced by triggers.
CREATE TABLE club_slug_registry (
    slug TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs (id) ON DELETE CASCADE
);

INSERT INTO club_slug_registry (slug, club_id)
SELECT slug, id FROM clubs;

CREATE OR REPLACE FUNCTION club_slug_registry_on_clubs_insert() RETURNS trigger AS $$
BEGIN
    INSERT INTO club_slug_registry (slug, club_id) VALUES (NEW.slug, NEW.id);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_clubs_slug_registry_insert
AFTER INSERT ON clubs
FOR EACH ROW
EXECUTE FUNCTION club_slug_registry_on_clubs_insert();

CREATE OR REPLACE FUNCTION club_slug_registry_on_clubs_update() RETURNS trigger AS $$
BEGIN
    IF OLD.slug IS DISTINCT FROM NEW.slug THEN
        -- Only delete the old slug from registry if it's NOT in history for this club
        -- (if it's in history, it must stay reserved)
        DELETE FROM club_slug_registry 
        WHERE slug = OLD.slug AND club_id = OLD.id
        AND NOT EXISTS (
            SELECT 1 FROM club_slug_history 
            WHERE old_slug = OLD.slug AND club_id = OLD.id
        );
        -- Insert new slug only if not already registered for this club
        INSERT INTO club_slug_registry (slug, club_id) VALUES (NEW.slug, NEW.id)
        ON CONFLICT (slug) DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_clubs_slug_registry_update
AFTER UPDATE OF slug ON clubs
FOR EACH ROW
EXECUTE FUNCTION club_slug_registry_on_clubs_update();

CREATE OR REPLACE FUNCTION club_slug_registry_on_history_insert() RETURNS trigger AS $$
BEGIN
    -- Only insert if not already registered for this club (avoids duplicate on rename)
    INSERT INTO club_slug_registry (slug, club_id) VALUES (NEW.old_slug, NEW.club_id)
    ON CONFLICT (slug) DO NOTHING;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_club_slug_history_registry_insert
AFTER INSERT ON club_slug_history
FOR EACH ROW
EXECUTE FUNCTION club_slug_registry_on_history_insert();

CREATE OR REPLACE FUNCTION club_slug_registry_on_history_delete() RETURNS trigger AS $$
BEGIN
    -- Only delete from registry if the slug is NOT the current slug for this club
    DELETE FROM club_slug_registry 
    WHERE slug = OLD.old_slug AND club_id = OLD.club_id
    AND NOT EXISTS (
        SELECT 1 FROM clubs 
        WHERE id = OLD.club_id AND slug = OLD.old_slug
    );
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_club_slug_history_registry_delete
AFTER DELETE ON club_slug_history
FOR EACH ROW
EXECUTE FUNCTION club_slug_registry_on_history_delete();