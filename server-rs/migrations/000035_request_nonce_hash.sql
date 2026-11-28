-- L-81: optional client nonce binding for magic login tokens (nullable, no table rewrite).
-- `ADD COLUMN IF NOT EXISTS` takes `ACCESS EXCLUSIVE` briefly on `magic_login_tokens`.
ALTER TABLE magic_login_tokens
    ADD COLUMN IF NOT EXISTS request_nonce_hash TEXT;