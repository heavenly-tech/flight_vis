CREATE TABLE clubs (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    checkin_valid_minutes INTEGER NOT NULL DEFAULT 30,
    checkin_hmac_secret TEXT NOT NULL DEFAULT ''
);

CREATE TABLE aircraft_types (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    code TEXT NOT NULL,
    UNIQUE (club_id, code)
);

CREATE TABLE aircraft (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    tail TEXT NOT NULL,
    type_id TEXT NOT NULL REFERENCES aircraft_types(id),
    active BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (club_id, tail)
);

CREATE TABLE pilots (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    license_no TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    role TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE (club_id, email)
);

CREATE TABLE pilot_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    expires_at BIGINT NOT NULL
);

CREATE TABLE checkouts (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    type_id TEXT NOT NULL REFERENCES aircraft_types(id),
    aircraft_id TEXT REFERENCES aircraft(id),
    expires_at BIGINT
);

CREATE TABLE devices (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL
);

CREATE TABLE device_pair_tokens (
    device_id TEXT PRIMARY KEY NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    token TEXT NOT NULL
);

CREATE TABLE flights (
    id TEXT PRIMARY KEY NOT NULL,
    pilot_id TEXT NOT NULL REFERENCES pilots(id),
    aircraft_id TEXT NOT NULL REFERENCES aircraft(id),
    status TEXT NOT NULL,
    started_at BIGINT NOT NULL,
    ended_at BIGINT,
    last_fix_at BIGINT,
    visibility TEXT NOT NULL DEFAULT 'club',
    show_pilot_name_public BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE flight_devices (
    flight_id TEXT NOT NULL REFERENCES flights(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id),
    PRIMARY KEY (flight_id, device_id)
);

CREATE TABLE fixes (
    id BIGSERIAL PRIMARY KEY,
    flight_id TEXT NOT NULL REFERENCES flights(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id),
    t_ms BIGINT NOT NULL,
    received_at BIGINT NOT NULL,
    lat DOUBLE PRECISION NOT NULL,
    lon DOUBLE PRECISION NOT NULL,
    alt DOUBLE PRECISION,
    alt_ref TEXT,
    speed DOUBLE PRECISION,
    course DOUBLE PRECISION,
    accuracy DOUBLE PRECISION,
    UNIQUE (flight_id, device_id, t_ms)
);

CREATE TABLE admin_map_prefs (
    pilot_id TEXT PRIMARY KEY NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    tails_json TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE display_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    created_by_pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    revoked_at BIGINT
);

CREATE TABLE checkins (
    id TEXT PRIMARY KEY NOT NULL,
    club_id TEXT NOT NULL REFERENCES clubs(id),
    pilot_id TEXT REFERENCES pilots(id),
    device_id TEXT REFERENCES devices(id),
    time_step BIGINT NOT NULL,
    token_digest TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT,
    consumed_at BIGINT,
    outcome TEXT NOT NULL,
    fail_reason TEXT
);

CREATE INDEX idx_fixes_flight_t ON fixes (flight_id, t_ms);
CREATE INDEX idx_fixes_received ON fixes (flight_id, received_at);
CREATE INDEX idx_flights_status ON flights (status);
CREATE INDEX idx_checkins_replay
    ON checkins (pilot_id, device_id, token_digest)
    WHERE outcome = 'ok';
CREATE INDEX idx_pilots_email_lower ON pilots (club_id, lower(email));
