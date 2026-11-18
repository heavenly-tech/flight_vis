ALTER TABLE clubs ADD COLUMN status TEXT NOT NULL DEFAULT 'live';
ALTER TABLE clubs ADD COLUMN contact_email TEXT NOT NULL DEFAULT '';
ALTER TABLE clubs ADD COLUMN email_verified_at INTEGER;
ALTER TABLE clubs ADD COLUMN suspended_at INTEGER;

UPDATE clubs SET contact_email = '' WHERE contact_email IS NULL;
UPDATE clubs SET status = 'live' WHERE status IS NULL OR status = '';

CREATE TABLE IF NOT EXISTS magic_login_tokens (
    id TEXT PRIMARY KEY NOT NULL,
    email TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER,
    purpose TEXT NOT NULL DEFAULT 'login'
);

CREATE INDEX IF NOT EXISTS idx_magic_login_email
    ON magic_login_tokens (email, created_at DESC);

CREATE TABLE IF NOT EXISTS webauthn_credentials (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    credential_id BLOB NOT NULL,
    public_key BLOB NOT NULL,
    counter INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE (credential_id)
);