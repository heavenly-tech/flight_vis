ALTER TABLE devices ADD COLUMN IF NOT EXISTS install_id TEXT;
ALTER TABLE devices ADD COLUMN IF NOT EXISTS model TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_pilot_install
    ON devices (pilot_id, install_id)
    WHERE install_id IS NOT NULL;