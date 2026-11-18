//! Idempotent migration from legacy shared-password club identity to a club-auth pilot.
//!
//! ## Postgres “legacy-owned” rows
//!
//! **Source pilots** (`legacy_ids`): only pilots named explicitly via CLI
//! `--source-pilot-id` (repeatable) and/or `--source-email` (repeatable), each resolved in the
//! target club. `FLIGHT_LEGACY_MIGRATE_SOURCE_EMAIL` adds one `--source-email`. No automatic
//! selection (no unverified-email heuristic).
//!
//! **Rows reassigned** (only where `pilot_id` / `created_by` / etc. ∈ `legacy_ids`):
//! flights, devices, checkouts, checkins, pilot_sessions, flight_shares.created_by,
//! access_requests, audit_log.actor_pilot_id, admin_map_prefs, display_sessions,
//! webauthn_credentials, guest_sessions — never rows owned by other pilot ids or clubs.
//!
//! **Disk library**: every `{FLIGHT_DATA}/flights/*` storage slug gets a `disk_flight_owners`
//! row (`owner_id`, target pilot id today) in the target club; files are not moved.

use crate::{
    club::{
        database::{connect_pool, run_migrations},
        disk_owner::{INSERT_DISK_FLIGHT_OWNER, INSERT_DISK_LIVE_SESSION_OWNER},
        new_id, now_ms,
    },
    demo_flight::{DEMO_FLIGHT_SLUG, LEGACY_DEMO_SLUG},
    live::LivePersist,
    store::Store,
};
use sqlx::{PgPool, Row};
use std::path::PathBuf;
use std::{collections::BTreeMap, path::Path};

type CountMap = BTreeMap<&'static str, i64>;

#[derive(Debug, Default, Clone)]
pub struct MigrateCounts {
    pub rows: CountMap,
}

impl MigrateCounts {
    pub fn total(&self) -> i64 {
        self.rows.values().sum()
    }

    pub fn print_report(&self, title: &str) {
        eprintln!("{title}");
        for (table, n) in &self.rows {
            eprintln!("  {table}: {n}");
        }
        eprintln!("  total: {}", self.total());
    }
}

#[derive(Debug)]
pub enum MigrateError {
    Usage(String),
    Db(String),
    Store(String),
    ClubNotFound(String),
    TargetExistsOnOtherClub,
    SourceNotFound(String),
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(m) => write!(f, "{m}"),
            Self::Db(m) => write!(f, "{m}"),
            Self::Store(m) => write!(f, "{m}"),
            Self::ClubNotFound(s) => write!(f, "club not found: {s}"),
            Self::TargetExistsOnOtherClub => {
                write!(f, "target email already registered on another club")
            }
            Self::SourceNotFound(m) => write!(f, "{m}"),
        }
    }
}

pub fn parse_cli_args(args: &[String]) -> Result<LegacyMigrateOpts, MigrateError> {
    let mut email = None;
    let mut club = None;
    let mut apply = false;
    let mut dry_run = false;
    let mut source_pilot_ids = Vec::new();
    let mut source_emails = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--email" => {
                i += 1;
                email = Some(
                    args.get(i)
                        .ok_or_else(|| MigrateError::Usage("--email requires a value".into()))?
                        .clone(),
                );
            }
            "--club" => {
                i += 1;
                club = Some(
                    args.get(i)
                        .ok_or_else(|| MigrateError::Usage("--club requires a value".into()))?
                        .clone(),
                );
            }
            "--source-pilot-id" => {
                i += 1;
                let id = args
                    .get(i)
                    .ok_or_else(|| {
                        MigrateError::Usage("--source-pilot-id requires a value".into())
                    })?
                    .trim()
                    .to_string();
                if !id.is_empty() {
                    source_pilot_ids.push(id);
                }
            }
            "--source-email" => {
                i += 1;
                let src = args
                    .get(i)
                    .ok_or_else(|| MigrateError::Usage("--source-email requires a value".into()))?
                    .trim()
                    .to_string();
                if !src.is_empty() {
                    source_emails.push(src);
                }
            }
            "--apply" => apply = true,
            "--dry-run" => dry_run = true,
            flag => {
                return Err(MigrateError::Usage(format!("unknown flag: {flag}")));
            }
        }
        i += 1;
    }
    let email = email.ok_or_else(|| MigrateError::Usage("--email is required".into()))?;
    let club = club.ok_or_else(|| MigrateError::Usage("--club is required".into()))?;
    if email.trim().is_empty() {
        return Err(MigrateError::Usage("--email must not be empty".into()));
    }
    if apply && dry_run {
        return Err(MigrateError::Usage(
            "use only one of --dry-run or --apply".into(),
        ));
    }
    if let Some(env_src) = optional_source_email() {
        let norm = env_src.trim().to_ascii_lowercase();
        if !norm.is_empty()
            && !source_emails
                .iter()
                .any(|e| e.trim().eq_ignore_ascii_case(&norm))
        {
            source_emails.push(env_src);
        }
    }
    if source_pilot_ids.is_empty() && source_emails.is_empty() {
        return Err(MigrateError::Usage(
            "at least one of --source-pilot-id or --source-email is required (or set FLIGHT_LEGACY_MIGRATE_SOURCE_EMAIL)".into(),
        ));
    }
    let dry_run = if apply { false } else { true };
    Ok(LegacyMigrateOpts {
        email: email.trim().to_string(),
        club_slug: club.trim().to_string(),
        dry_run,
        source_pilot_ids,
        source_emails,
    })
}

#[derive(Debug, Clone)]
pub struct LegacyMigrateOpts {
    pub email: String,
    pub club_slug: String,
    pub dry_run: bool,
    pub source_pilot_ids: Vec<String>,
    pub source_emails: Vec<String>,
}

async fn club_id_for_slug(pool: &PgPool, slug: &str) -> Result<String, MigrateError> {
    sqlx::query_scalar("SELECT id FROM clubs WHERE slug = $1")
        .bind(slug)
        .fetch_optional(pool)
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))
        .and_then(|id| id.ok_or_else(|| MigrateError::ClubNotFound(slug.to_string())))
}

fn optional_source_email() -> Option<String> {
    std::env::var("FLIGHT_LEGACY_MIGRATE_SOURCE_EMAIL")
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

struct SourcePilotRow {
    id: String,
    email: String,
    club_slug: String,
}

async fn resolve_legacy_owner_ids(
    pool: &PgPool,
    club_id: &str,
    opts: &LegacyMigrateOpts,
) -> Result<Vec<String>, MigrateError> {
    if opts.source_pilot_ids.is_empty() && opts.source_emails.is_empty() {
        return Err(MigrateError::Usage(
            "at least one of --source-pilot-id or --source-email is required".into(),
        ));
    }
    let mut ids = Vec::new();
    for id in &opts.source_pilot_ids {
        let found: Option<String> =
            sqlx::query_scalar("SELECT id FROM pilots WHERE id = $1 AND club_id = $2")
                .bind(id)
                .bind(club_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| MigrateError::Db(e.to_string()))?;
        if found.is_none() {
            return Err(MigrateError::SourceNotFound(format!(
                "source pilot id {id} not found in club"
            )));
        }
        ids.push(id.clone());
    }
    for email in &opts.source_emails {
        let norm = email.trim();
        let found: Option<String> = sqlx::query_scalar(
            "SELECT id FROM pilots WHERE club_id = $1 AND lower(trim(email)) = lower(trim($2))",
        )
        .bind(club_id)
        .bind(norm)
        .fetch_optional(pool)
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;
        let Some(found) = found else {
            return Err(MigrateError::SourceNotFound(format!(
                "source email {norm} not found in club"
            )));
        };
        ids.push(found);
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

async fn fetch_source_pilot_rows(
    pool: &PgPool,
    legacy_ids: &[String],
) -> Result<Vec<SourcePilotRow>, MigrateError> {
    if legacy_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT p.id, COALESCE(p.email, ''), c.slug
         FROM pilots p
         JOIN clubs c ON c.id = p.club_id
         WHERE p.id = ANY($1)
         ORDER BY p.id",
    )
    .bind(legacy_ids)
    .fetch_all(pool)
    .await
    .map_err(|e| MigrateError::Db(e.to_string()))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(SourcePilotRow {
                id: r.try_get(0).ok()?,
                email: r.try_get(1).ok()?,
                club_slug: r.try_get(2).ok()?,
            })
        })
        .collect())
}

fn print_source_pilots(rows: &[SourcePilotRow]) {
    if rows.is_empty() {
        eprintln!("  (no source pilots resolved)");
        return;
    }
    for p in rows {
        eprintln!(
            "  source pilot: id={} email={} club={}",
            p.id, p.email, p.club_slug
        );
    }
}

async fn print_migration_plan(
    pool: &PgPool,
    legacy_ids: &[String],
    counts: &MigrateCounts,
    title: &str,
) -> Result<(), MigrateError> {
    eprintln!("{title}");
    let sources = fetch_source_pilot_rows(pool, legacy_ids).await?;
    print_source_pilots(&sources);
    for (table, n) in &counts.rows {
        eprintln!("  {table}: {n}");
    }
    eprintln!("  total: {}", counts.total());
    Ok(())
}

async fn count_rows(pool: &PgPool, legacy_ids: &[String]) -> Result<MigrateCounts, MigrateError> {
    if legacy_ids.is_empty() {
        return Ok(MigrateCounts::default());
    }
    let mut counts = CountMap::new();
    macro_rules! count {
        ($label:expr, $sql:expr) => {{
            let n: i64 = sqlx::query_scalar($sql)
                .bind(legacy_ids)
                .fetch_one(pool)
                .await
                .map_err(|e| MigrateError::Db(e.to_string()))?;
            if n > 0 {
                counts.insert($label, n);
            }
        }};
    }
    count!(
        "flights.pilot_id",
        "SELECT COUNT(*) FROM flights WHERE pilot_id = ANY($1)"
    );
    count!(
        "fixes (via flights)",
        "SELECT COUNT(*) FROM fixes WHERE flight_id IN (SELECT id FROM flights WHERE pilot_id = ANY($1))"
    );
    count!(
        "flight_devices (via flights)",
        "SELECT COUNT(*) FROM flight_devices WHERE flight_id IN (SELECT id FROM flights WHERE pilot_id = ANY($1))"
    );
    count!(
        "devices.pilot_id",
        "SELECT COUNT(*) FROM devices WHERE pilot_id = ANY($1)"
    );
    count!(
        "checkouts.pilot_id",
        "SELECT COUNT(*) FROM checkouts WHERE pilot_id = ANY($1)"
    );
    count!(
        "checkins.pilot_id",
        "SELECT COUNT(*) FROM checkins WHERE pilot_id = ANY($1)"
    );
    count!(
        "pilot_sessions.pilot_id",
        "SELECT COUNT(*) FROM pilot_sessions WHERE pilot_id = ANY($1)"
    );
    count!(
        "flight_shares.created_by",
        "SELECT COUNT(*) FROM flight_shares WHERE created_by = ANY($1)"
    );
    count!(
        "access_requests.pilot_id",
        "SELECT COUNT(*) FROM access_requests WHERE pilot_id = ANY($1)"
    );
    count!(
        "access_requests.decided_by_pilot_id",
        "SELECT COUNT(*) FROM access_requests WHERE decided_by_pilot_id = ANY($1)"
    );
    count!(
        "audit_log.actor_pilot_id",
        "SELECT COUNT(*) FROM audit_log WHERE actor_pilot_id = ANY($1)"
    );
    count!(
        "admin_map_prefs.pilot_id",
        "SELECT COUNT(*) FROM admin_map_prefs WHERE pilot_id = ANY($1)"
    );
    count!(
        "display_sessions.created_by_pilot_id",
        "SELECT COUNT(*) FROM display_sessions WHERE created_by_pilot_id = ANY($1)"
    );
    count!(
        "webauthn_credentials.pilot_id",
        "SELECT COUNT(*) FROM webauthn_credentials WHERE pilot_id = ANY($1)"
    );
    count!(
        "guest_sessions.guest_pilot_id",
        "SELECT COUNT(*) FROM guest_sessions WHERE guest_pilot_id = ANY($1)"
    );
    Ok(MigrateCounts { rows: counts })
}

fn public_slug_for_storage(storage_slug: &str) -> String {
    if storage_slug == LEGACY_DEMO_SLUG {
        DEMO_FLIGHT_SLUG.to_string()
    } else {
        storage_slug.to_string()
    }
}

async fn count_disk_library(pool: &PgPool, data_dir: &Path) -> Result<MigrateCounts, MigrateError> {
    let store = Store::new(data_dir);
    let slugs = store.list_storage_slugs().await;
    let mut counts = CountMap::new();
    if !slugs.is_empty() {
        counts.insert("disk.flights", slugs.len() as i64);
    }
    let mut track_total = 0i64;
    let mut video_meta = 0i64;
    let mut video_bytes = 0i64;
    for slug in &slugs {
        let (t, v, vb) = store.flight_disk_stats(slug).await;
        track_total += t as i64;
        video_meta += v;
        video_bytes += vb as i64;
    }
    if track_total > 0 {
        counts.insert("disk.track_bytes", track_total);
    }
    if video_meta > 0 {
        counts.insert("disk.video_meta_rows", video_meta);
    }
    if video_bytes > 0 {
        counts.insert("disk.video_file_bytes", video_bytes);
    }
    let unowned = if slugs.is_empty() {
        0
    } else {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM unnest($1::text[]) AS s(slug)
             WHERE NOT EXISTS (SELECT 1 FROM disk_flight_owners d WHERE d.storage_slug = s.slug)",
        )
        .bind(&slugs)
        .fetch_one(pool)
        .await
        .unwrap_or(slugs.len() as i64)
    };
    if unowned > 0 {
        counts.insert("disk_flight_owners.pending", unowned);
    }
    let live = LivePersist::new(data_dir);
    if let Ok(index) = live.load_index().await {
        let n = index.len() as i64;
        if n > 0 {
            counts.insert("disk.live_sessions", n);
        }
        let mut log_bytes = 0i64;
        for id in index.keys() {
            let path = data_dir.join("live").join(format!("{id}.ndjson"));
            log_bytes += tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0) as i64;
        }
        if log_bytes > 0 {
            counts.insert("disk.live_log_bytes", log_bytes);
        }
        let claimed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM disk_live_session_owners")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        let pending_live = n - claimed;
        if pending_live > 0 {
            counts.insert("disk_live_session_owners.pending", pending_live);
        }
    }
    Ok(MigrateCounts { rows: counts })
}

async fn apply_disk_ownership(
    pool: &PgPool,
    data_dir: &Path,
    club_id: &str,
    target_id: &str,
) -> Result<i64, MigrateError> {
    let store = Store::new(data_dir);
    let slugs = store.list_storage_slugs().await;
    let mut changed = 0i64;
    let now = now_ms();
    for storage in slugs {
        let public = public_slug_for_storage(&storage);
        let r = sqlx::query(INSERT_DISK_FLIGHT_OWNER)
            .bind(&public)
            .bind(&storage)
            .bind(target_id)
            .bind(club_id)
            .bind(now)
            .execute(pool)
            .await
            .map_err(|e| MigrateError::Db(e.to_string()))?;
        changed += r.rows_affected() as i64;
    }
    let live = LivePersist::new(data_dir);
    if let Ok(index) = live.load_index().await {
        for (session_id, record) in index {
            let path = data_dir.join("live").join(format!("{session_id}.ndjson"));
            let log_bytes = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0) as i64;
            let r = sqlx::query(INSERT_DISK_LIVE_SESSION_OWNER)
                .bind(&session_id)
                .bind(target_id)
                .bind(club_id)
                .bind(&record.device)
                .bind(record.fix_count as i32)
                .bind(log_bytes)
                .bind(now)
                .execute(pool)
                .await
                .map_err(|e| MigrateError::Db(e.to_string()))?;
            changed += r.rows_affected() as i64;
        }
    }
    Ok(changed)
}

fn merge_counts(base: &mut MigrateCounts, extra: MigrateCounts) {
    for (k, v) in extra.rows {
        *base.rows.entry(k).or_insert(0) += v;
    }
}

async fn ensure_target_pilot(
    pool: &PgPool,
    club_id: &str,
    email: &str,
    grant_admin: bool,
) -> Result<String, MigrateError> {
    let email_norm = email.trim().to_ascii_lowercase();
    let existing: Option<(String, String)> =
        sqlx::query_as("SELECT id, club_id FROM pilots WHERE lower(email) = $1")
            .bind(&email_norm)
            .fetch_optional(pool)
            .await
            .map_err(|e| MigrateError::Db(e.to_string()))?;
    let pilot_id = if let Some((id, home)) = existing {
        if home != club_id {
            return Err(MigrateError::TargetExistsOnOtherClub);
        }
        id
    } else {
        let id = new_id();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at, email_verified_at)
             VALUES ($1, $2, $3, $4, '', 'approved', $5, '', $6, $6)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(email.trim())
        .bind(&email_norm)
        .bind(if grant_admin { "admin" } else { "pilot" })
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;
        id
    };
    let member_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM club_memberships WHERE pilot_id = $1 AND club_id = $2)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .fetch_one(pool)
    .await
    .map_err(|e| MigrateError::Db(e.to_string()))?;
    if !member_exists {
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, $4, 'approved', $5)",
        )
        .bind(new_id())
        .bind(&pilot_id)
        .bind(club_id)
        .bind(if grant_admin { "admin" } else { "pilot" })
        .bind(now_ms())
        .execute(pool)
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;
    } else if grant_admin {
        sqlx::query(
            "UPDATE club_memberships SET role = 'admin', status = 'approved'
             WHERE pilot_id = $1 AND club_id = $2 AND role <> 'admin'",
        )
        .bind(&pilot_id)
        .bind(club_id)
        .execute(pool)
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;
    }
    Ok(pilot_id)
}

async fn legacy_was_admin(pool: &PgPool, legacy_ids: &[String]) -> Result<bool, MigrateError> {
    if legacy_ids.is_empty() {
        return Ok(false);
    }
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE id = ANY($1) AND role = 'admin'")
            .bind(legacy_ids)
            .fetch_one(pool)
            .await
            .map_err(|e| MigrateError::Db(e.to_string()))?;
    Ok(n > 0)
}

async fn apply_row_updates(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    club_id: &str,
    legacy_ids: &[String],
    target_id: &str,
) -> Result<(), MigrateError> {
    macro_rules! run2 {
        ($sql:expr) => {
            sqlx::query($sql)
                .bind(target_id)
                .bind(legacy_ids)
                .execute(&mut **tx)
                .await
                .map_err(|e| MigrateError::Db(e.to_string()))?;
        };
    }
    macro_rules! run3 {
        ($sql:expr) => {
            sqlx::query($sql)
                .bind(target_id)
                .bind(legacy_ids)
                .bind(club_id)
                .execute(&mut **tx)
                .await
                .map_err(|e| MigrateError::Db(e.to_string()))?;
        };
    }
    run3!(
        "UPDATE flights SET pilot_id = $1, club_id = COALESCE(club_id, $3) WHERE pilot_id = ANY($2)"
    );
    run2!("UPDATE devices SET pilot_id = $1 WHERE pilot_id = ANY($2)");
    run3!(
        "UPDATE checkouts SET pilot_id = $1, club_id = COALESCE(club_id, $3) WHERE pilot_id = ANY($2)"
    );
    run2!("UPDATE checkins SET pilot_id = $1 WHERE pilot_id = ANY($2)");
    run2!("UPDATE pilot_sessions SET pilot_id = $1 WHERE pilot_id = ANY($2)");
    run2!("UPDATE flight_shares SET created_by = $1 WHERE created_by = ANY($2)");
    run2!("UPDATE access_requests SET pilot_id = $1 WHERE pilot_id = ANY($2)");
    run2!(
        "UPDATE access_requests SET decided_by_pilot_id = $1 WHERE decided_by_pilot_id = ANY($2)"
    );
    run2!("UPDATE audit_log SET actor_pilot_id = $1 WHERE actor_pilot_id = ANY($2)");
    sqlx::query(
        "UPDATE admin_map_prefs SET pilot_id = $1 WHERE pilot_id = ANY($2)
         AND NOT EXISTS (SELECT 1 FROM admin_map_prefs WHERE pilot_id = $1)",
    )
    .bind(target_id)
    .bind(legacy_ids)
    .execute(&mut **tx)
    .await
    .map_err(|e| MigrateError::Db(e.to_string()))?;
    run2!(
        "UPDATE display_sessions SET created_by_pilot_id = $1 WHERE created_by_pilot_id = ANY($2)"
    );
    run2!("UPDATE webauthn_credentials SET pilot_id = $1 WHERE pilot_id = ANY($2)");
    run2!("UPDATE guest_sessions SET guest_pilot_id = $1 WHERE guest_pilot_id = ANY($2)");
    Ok(())
}

pub async fn gather_counts(
    pool: &PgPool,
    data_dir: &Path,
    opts: &LegacyMigrateOpts,
) -> Result<MigrateCounts, MigrateError> {
    let club_id = club_id_for_slug(pool, &opts.club_slug).await?;
    let legacy = resolve_legacy_owner_ids(pool, &club_id, opts).await?;
    let mut counts = count_rows(pool, &legacy).await?;
    merge_counts(&mut counts, count_disk_library(pool, data_dir).await?);
    Ok(counts)
}

pub async fn run_legacy_migrate(
    opts: &LegacyMigrateOpts,
    data_dir: &Path,
) -> Result<MigrateCounts, MigrateError> {
    let pool = connect_pool()
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;
    run_migrations(&pool)
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;
    let club_id = club_id_for_slug(&pool, &opts.club_slug).await?;
    let legacy_ids = resolve_legacy_owner_ids(&pool, &club_id, opts).await?;
    let mut counts = count_rows(&pool, &legacy_ids).await?;
    merge_counts(&mut counts, count_disk_library(&pool, data_dir).await?);

    let plan_title = if opts.dry_run {
        "migrate-legacy dry-run (no writes)"
    } else {
        "migrate-legacy apply (pre-commit)"
    };
    print_migration_plan(&pool, &legacy_ids, &counts, plan_title).await?;

    if opts.dry_run {
        return Ok(counts);
    }

    let disk_pending = counts
        .rows
        .get("disk_flight_owners.pending")
        .copied()
        .unwrap_or(0)
        + counts
            .rows
            .get("disk_live_session_owners.pending")
            .copied()
            .unwrap_or(0);
    if legacy_ids.is_empty() && counts.total() == disk_pending && disk_pending == 0 {
        eprintln!("migrate-legacy: no changes");
        return Ok(counts);
    }

    let admin = legacy_was_admin(&pool, &legacy_ids).await?;
    let target_id = ensure_target_pilot(&pool, &club_id, &opts.email, admin).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;
    if !legacy_ids.is_empty() {
        apply_row_updates(&mut tx, &club_id, &legacy_ids, &target_id).await?;
    }
    let audit_detail = serde_json::json!({
        "targetEmail": opts.email,
        "targetPilotId": target_id,
        "legacyPilotIds": legacy_ids,
        "counts": counts.rows,
    })
    .to_string();
    sqlx::query(
        "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
         VALUES ($1, $2, $3, 'legacy_migrate', 'pilot', $4, $5, $6)",
    )
    .bind(new_id())
    .bind(&club_id)
    .bind(&target_id)
    .bind(&target_id)
    .bind(&audit_detail)
    .bind(now_ms())
    .execute(&mut *tx)
    .await
    .map_err(|e| MigrateError::Db(e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| MigrateError::Db(e.to_string()))?;

    apply_disk_ownership(&pool, data_dir, &club_id, &target_id).await?;

    let after_legacy = count_rows(&pool, &legacy_ids).await?;
    let after_disk = count_disk_library(&pool, data_dir).await?;
    let mut post_pg = MigrateCounts::default();
    merge_counts(&mut post_pg, after_legacy);
    print_migration_plan(
        &pool,
        &legacy_ids,
        &post_pg,
        "migrate-legacy post-commit (postgres legacy owners)",
    )
    .await?;
    after_disk.print_report("migrate-legacy post-commit (disk)");
    let pending: i64 = after_disk
        .rows
        .get("disk_flight_owners.pending")
        .copied()
        .unwrap_or(0)
        + after_disk
            .rows
            .get("disk_live_session_owners.pending")
            .copied()
            .unwrap_or(0);
    if after_legacy.total() != 0 || pending != 0 {
        return Err(MigrateError::Db(
            "post-commit legacy row counts did not reach zero".into(),
        ));
    }
    Ok(counts)
}

pub async fn run_legacy_migrate_cli(args: &[String]) -> i32 {
    let opts = match parse_cli_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("migrate-legacy: {e}");
            eprintln!(
                "usage: flight_vis migrate-legacy --email <addr> --club <slug> \\
  (--source-pilot-id <id> | --source-email <addr> | FLIGHT_LEGACY_MIGRATE_SOURCE_EMAIL)+ \\
  [--dry-run|--apply]"
            );
            return 2;
        }
    };
    let data_dir = std::env::var("FLIGHT_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"));
    match run_legacy_migrate(&opts, &data_dir).await {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("migrate-legacy: {e}");
            1
        }
    }
}
