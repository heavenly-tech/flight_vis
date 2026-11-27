-- E2: Club-less flight shares (nullable flight_shares.club_id), defensive fixes dedup
-- constraint guard, and partial index for active public flight readers.

-- 1. Allow club-less flight share links (club_id NULL when flight has no club).
ALTER TABLE flight_shares
    ALTER COLUMN club_id DROP NOT NULL;

-- 2. Defensive deduplication of identical (flight_id, device_id, t_ms) fixes (no-op when
--    000001's UNIQUE constraint is already intact; safe on populated DBs with legacy duplicates)
--    and ensure a unique index/constraint exists for ON CONFLICT (flight_id, device_id, t_ms).
DELETE FROM fixes a
USING fixes b
WHERE a.flight_id = b.flight_id
  AND a.device_id = b.device_id
  AND a.t_ms = b.t_ms
  AND a.id > b.id;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_index i
        JOIN pg_class t ON t.oid = i.indrelid
        JOIN pg_namespace n ON n.oid = t.relnamespace
        WHERE n.nspname = current_schema()
          AND t.relname = 'fixes'
          AND i.indisunique
          AND i.indpred IS NULL
          AND (
              SELECT array_agg(a.attname::text ORDER BY k.ord)
              FROM unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord)
              JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum
              WHERE k.ord <= i.indnkeyatts
          ) = ARRAY['flight_id', 'device_id', 't_ms']
    ) THEN
        CREATE UNIQUE INDEX idx_fixes_flight_device_t_unique
            ON fixes (flight_id, device_id, t_ms);
    END IF;
END $$;

-- 3. Speed up public/nearby live-flight scans (both club and club-less active public flights).
CREATE INDEX IF NOT EXISTS idx_flights_active_public_last_fix
    ON flights (last_fix_at DESC, id ASC)
    WHERE status = 'active' AND ended_at IS NULL AND visibility = 'public';
