-- Idempotent backfill for legacy rows after email_verified_at was introduced (000011).

UPDATE clubs
SET email_verified_at = created_at
WHERE status = 'live' AND email_verified_at IS NULL;

UPDATE pilots
SET email_verified_at = created_at
WHERE status = 'approved' AND email_verified_at IS NULL;