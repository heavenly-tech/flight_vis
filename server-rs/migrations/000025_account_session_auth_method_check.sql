-- Enforce account session auth_method values (SQLite-safe table rebuild pattern).
UPDATE account_sessions
SET auth_method = 'password'
WHERE auth_method IS NULL
   OR auth_method NOT IN ('magic', 'password');

CREATE TABLE account_sessions_new (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    active_membership_id TEXT REFERENCES club_memberships(id) ON DELETE SET NULL,
    expires_at BIGINT NOT NULL,
    compat_pilot_session TEXT,
    created_at BIGINT NOT NULL DEFAULT 0,
    auth_method TEXT NOT NULL DEFAULT 'password' CHECK (auth_method IN ('magic', 'password'))
);

INSERT INTO account_sessions_new (
    id,
    account_id,
    active_membership_id,
    expires_at,
    compat_pilot_session,
    created_at,
    auth_method
)
SELECT
    id,
    account_id,
    active_membership_id,
    expires_at,
    compat_pilot_session,
    coalesce(created_at, 0),
    auth_method
FROM account_sessions;

DROP TABLE account_sessions;

ALTER TABLE account_sessions_new RENAME TO account_sessions;

CREATE INDEX IF NOT EXISTS idx_account_sessions_account ON account_sessions (account_id);