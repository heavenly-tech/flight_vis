ALTER TABLE clubs ADD COLUMN checkin_window_hours INTEGER NOT NULL DEFAULT 12;

UPDATE clubs SET checkin_window_hours = 12;