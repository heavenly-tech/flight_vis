-- PR1b (#24): pending signup passwords live on magic tokens, never on unverified accounts.
ALTER TABLE magic_login_tokens
    ADD COLUMN IF NOT EXISTS pending_password_hash TEXT;

ALTER TABLE magic_login_tokens
    ADD COLUMN IF NOT EXISTS signup_nonce_hash TEXT;