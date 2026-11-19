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
        DELETE FROM club_slug_registry WHERE slug = OLD.slug AND club_id = OLD.id;
        INSERT INTO club_slug_registry (slug, club_id) VALUES (NEW.slug, NEW.id);
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
    INSERT INTO club_slug_registry (slug, club_id) VALUES (NEW.old_slug, NEW.club_id);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_club_slug_history_registry_insert
AFTER INSERT ON club_slug_history
FOR EACH ROW
EXECUTE FUNCTION club_slug_registry_on_history_insert();

CREATE OR REPLACE FUNCTION club_slug_registry_on_history_delete() RETURNS trigger AS $$
BEGIN
    DELETE FROM club_slug_registry WHERE slug = OLD.old_slug AND club_id = OLD.club_id;
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_club_slug_history_registry_delete
AFTER DELETE ON club_slug_history
FOR EACH ROW
EXECUTE FUNCTION club_slug_registry_on_history_delete();