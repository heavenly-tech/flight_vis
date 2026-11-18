//! Disk library rows key ownership by `owner_id` (pilots.id today; global accounts.id later).
//! All SQL touching that column should go through the statements in this module.

/// Column name for the disk-library owner principal.
pub(crate) const OWNER_COL: &str = "owner_id";

pub(crate) const INSERT_DISK_FLIGHT_OWNER: &str =
    "INSERT INTO disk_flight_owners (public_slug, storage_slug, owner_id, club_id, migrated_at)
     VALUES ($1, $2, $3, $4, $5)
     ON CONFLICT (public_slug) DO NOTHING";

pub(crate) const INSERT_DISK_LIVE_SESSION_OWNER: &str =
    "INSERT INTO disk_live_session_owners (session_id, owner_id, club_id, device_label, fix_count, log_bytes, migrated_at)
     VALUES ($1, $2, $3, $4, $5, $6, $7)
     ON CONFLICT (session_id) DO NOTHING";