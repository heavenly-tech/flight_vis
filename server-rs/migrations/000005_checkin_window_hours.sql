ALTER TABLE clubs ADD COLUMN IF NOT EXISTS checkin_window_hours INTEGER NOT NULL DEFAULT 12;

UPDATE clubs SET checkin_window_hours = 12
WHERE checkin_window_hours IS NULL OR checkin_window_hours < 1;