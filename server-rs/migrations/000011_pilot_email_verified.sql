-- Pilot email ownership verification (register/claim).

ALTER TABLE pilots ADD COLUMN IF NOT EXISTS email_verified_at BIGINT;

ALTER TABLE magic_login_tokens ADD COLUMN IF NOT EXISTS pilot_id TEXT REFERENCES pilots(id) ON DELETE CASCADE;