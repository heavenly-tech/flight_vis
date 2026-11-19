ALTER TABLE account_sessions
    ADD COLUMN IF NOT EXISTS created_at BIGINT NOT NULL DEFAULT 0;

ALTER TABLE account_sessions
    ADD COLUMN IF NOT EXISTS auth_method TEXT NOT NULL DEFAULT 'password';