-- Case-insensitive pilot email per club (no citext extension).
ALTER TABLE pilots DROP CONSTRAINT IF EXISTS pilots_club_id_email_key;
DROP INDEX IF EXISTS idx_pilots_email_lower;
CREATE UNIQUE INDEX pilots_club_id_email_lower_key ON pilots (club_id, lower(email));