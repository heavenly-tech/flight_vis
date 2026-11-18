-- PR1b (#24): lands as 000014 after #19 (000013). #20 → 000015, #22 → 000016 when retargeted.
-- Global accounts, account sessions, membership authority, backfill from pilots.
-- Account ids are random (gen_random_uuid); idempotent by lower(email), not by id.
-- Backfill password rule: per lower(email), non-empty password_hash wins; ties broken by latest created_at.
-- Guest pilots (@guest.invalid) and status=guest stay account_id NULL.

CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY NOT NULL,
    email TEXT NOT NULL,
    password_hash TEXT,
    email_verified_at BIGINT,
    global_role TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    suspended_at BIGINT
);

CREATE UNIQUE INDEX IF NOT EXISTS accounts_email_lower_key ON accounts (lower(email));

CREATE TABLE IF NOT EXISTS account_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    active_membership_id TEXT REFERENCES club_memberships(id) ON DELETE SET NULL,
    expires_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_account_sessions_account ON account_sessions (account_id);

ALTER TABLE pilots ADD COLUMN IF NOT EXISTS account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_pilots_account_id ON pilots (account_id);

ALTER TABLE magic_login_tokens
    ADD COLUMN IF NOT EXISTS account_id TEXT REFERENCES accounts(id) ON DELETE CASCADE;

ALTER TABLE flights ADD COLUMN IF NOT EXISTS owner_account_id TEXT REFERENCES accounts(id);

INSERT INTO accounts (id, email, password_hash, email_verified_at, global_role, created_at, suspended_at)
SELECT
    replace(gen_random_uuid()::text, '-', ''),
    email_lower,
    password_hash,
    email_verified_at,
    global_role,
    created_at,
    NULL::bigint
FROM (
    SELECT
        lower(trim(email)) AS email_lower,
        first_value(password_hash) OVER (
            PARTITION BY lower(trim(email))
            ORDER BY
                (CASE WHEN coalesce(password_hash, '') <> '' THEN 1 ELSE 0 END) DESC,
                created_at DESC
        ) AS password_hash,
        max(email_verified_at) OVER (PARTITION BY lower(trim(email))) AS email_verified_at,
        (CASE
            WHEN max(CASE WHEN global_role = 'superadmin' THEN 1 ELSE 0 END)
                 OVER (PARTITION BY lower(trim(email))) > 0
            THEN 'superadmin'
            ELSE ''
        END) AS global_role,
        min(created_at) OVER (PARTITION BY lower(trim(email))) AS created_at,
        row_number() OVER (PARTITION BY lower(trim(email)) ORDER BY created_at ASC) AS rn
    FROM pilots
    WHERE status <> 'guest'
      AND email NOT LIKE 'guest-%@guest.invalid'
      AND trim(email) <> ''
) s
WHERE rn = 1
  AND NOT EXISTS (SELECT 1 FROM accounts a WHERE lower(a.email) = s.email_lower);

UPDATE pilots p
SET account_id = a.id
FROM accounts a
WHERE lower(trim(p.email)) = lower(a.email)
  AND p.status <> 'guest'
  AND p.email NOT LIKE 'guest-%@guest.invalid'
  AND (p.account_id IS NULL OR p.account_id <> a.id);

INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
SELECT
    replace(gen_random_uuid()::text, '-', ''),
    p.id,
    p.club_id,
    CASE p.role
        WHEN 'admin' THEN 'admin'
        WHEN 'staff' THEN 'staff'
        ELSE 'pilot'
    END,
    CASE
        WHEN p.status = 'approved' THEN 'active'
        WHEN p.status = 'suspended' THEN 'suspended'
        WHEN p.status = 'guest' THEN 'pending'
        ELSE 'pending'
    END,
    coalesce(p.created_at, 0)
FROM pilots p
WHERE p.status <> 'guest'
  AND p.email NOT LIKE 'guest-%@guest.invalid'
ON CONFLICT (pilot_id, club_id) DO UPDATE SET
    role = CASE
        WHEN EXCLUDED.role IN ('admin', 'staff', 'pilot') THEN EXCLUDED.role
        ELSE club_memberships.role
    END,
    status = CASE
        WHEN club_memberships.status IN ('active', 'suspended') THEN club_memberships.status
        ELSE EXCLUDED.status
    END;

UPDATE club_memberships SET status = 'active' WHERE status = 'approved';