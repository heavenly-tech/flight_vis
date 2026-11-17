CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY NOT NULL
);

CREATE TABLE IF NOT EXISTS clubs (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS aircraft_types (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    code TEXT NOT NULL,
    UNIQUE (club_id, code)
);

CREATE TABLE IF NOT EXISTS aircraft (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    tail TEXT NOT NULL,
    type_id TEXT NOT NULL REFERENCES aircraft_types(id),
    active INTEGER NOT NULL DEFAULT 1,
    UNIQUE (club_id, tail)
);

CREATE TABLE IF NOT EXISTS pilots (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    license_no TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    role TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (club_id, email)
);

CREATE TABLE IF NOT EXISTS pilot_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS checkouts (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    type_id TEXT NOT NULL REFERENCES aircraft_types(id),
    aircraft_id TEXT REFERENCES aircraft(id),
    expires_at INTEGER
);

CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS flights (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id),
    aircraft_id TEXT NOT NULL REFERENCES aircraft(id),
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    last_fix_at INTEGER,
    visibility TEXT NOT NULL DEFAULT 'club',
    show_pilot_name_public INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS flight_devices (
    flight_id TEXT NOT NULL REFERENCES flights(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id),
    PRIMARY KEY (flight_id, device_id)
);

CREATE TABLE IF NOT EXISTS fixes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    flight_id TEXT NOT NULL REFERENCES flights(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id),
    t_ms INTEGER NOT NULL,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    alt REAL,
    speed REAL,
    course REAL,
    accuracy REAL,
  UNIQUE (flight_id, device_id, t_ms)
);

CREATE TABLE IF NOT EXISTS admin_map_prefs (
    pilot_id TEXT PRIMARY KEY NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    tails_json TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS idx_fixes_flight_t ON fixes (flight_id, t_ms);
CREATE INDEX IF NOT EXISTS idx_flights_status ON flights (status);