-- Follow-up to 000016: passwords only after email proof; membership status normalize.
UPDATE club_memberships SET status = 'active' WHERE status = 'approved';

UPDATE accounts a
SET password_hash = NULL
WHERE a.email_verified_at IS NULL
  AND coalesce(a.password_hash, '') <> '';

-- Refresh account passwords from approved, email-verified pilots only.
UPDATE accounts a
SET password_hash = sub.ph
FROM (
    SELECT
        lower(trim(p.email)) AS email_lower,
        first_value(p.password_hash) OVER (
            PARTITION BY lower(trim(p.email))
            ORDER BY p.created_at DESC
        ) AS ph
    FROM pilots p
    WHERE p.status = 'approved'
      AND p.email_verified_at IS NOT NULL
      AND coalesce(p.password_hash, '') <> ''
) sub
WHERE lower(a.email) = sub.email_lower
  AND a.email_verified_at IS NOT NULL
  AND coalesce(a.password_hash, '') = '';