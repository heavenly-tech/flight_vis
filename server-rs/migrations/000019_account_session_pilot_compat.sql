-- Reuse one compat pilot session per account session (PR1b follow-up).
ALTER TABLE account_sessions
    ADD COLUMN IF NOT EXISTS compat_pilot_session TEXT;
