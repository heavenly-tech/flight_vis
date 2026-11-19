-- Account-level UI locale; pilots.locale NULL inherits club default then Accept-Language.
-- Idempotent; numbered after 000016_accounts_memberships (#24).

ALTER TABLE accounts ADD COLUMN IF NOT EXISTS locale TEXT;

ALTER TABLE pilots ALTER COLUMN locale DROP DEFAULT;
ALTER TABLE pilots ALTER COLUMN locale DROP NOT NULL;