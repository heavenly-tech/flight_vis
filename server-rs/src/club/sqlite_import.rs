//! One-shot import of legacy `{FLIGHT_DATA}/club.sqlite` into Postgres.
use crate::club::database::{self, DbConnectError};
use rusqlite::{Connection, OpenFlags};
use sqlx::PgPool;
use std::path::Path;

pub(crate) async fn import_if_present(
    data_dir: &Path,
    pool: &PgPool,
) -> Result<(), DbConnectError> {
    let path = database::legacy_sqlite_path(data_dir);
    if !path.is_file() {
        return Ok(());
    }
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pilots")
        .fetch_one(pool)
        .await
        .map_err(DbConnectError::from)?;
    if existing > 0 {
        return Ok(());
    }
    let sqlite = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| DbConnectError::Sqlx(sqlx::Error::Configuration(e.to_string().into())))?;
    import_table_clubs(&sqlite, pool).await?;
    import_table_pilots(&sqlite, pool).await?;
    import_table_aircraft_types(&sqlite, pool).await?;
    import_table_aircraft(&sqlite, pool).await?;
    import_table_devices(&sqlite, pool).await?;
    import_table_device_pair_tokens(&sqlite, pool).await?;
    import_table_flights(&sqlite, pool).await?;
    import_table_fixes(&sqlite, pool).await?;
    Ok(())
}

fn sqlite_err(e: rusqlite::Error) -> DbConnectError {
    DbConnectError::Sqlx(sqlx::Error::Configuration(e.to_string().into()))
}

async fn import_table_clubs(sqlite: &Connection, pool: &PgPool) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare("SELECT id, name, created_at FROM clubs")
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(sqlite_err)?;
    for row in rows {
        let (id, name, created_at) = row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO clubs (id, name, created_at, checkin_valid_minutes, checkin_hmac_secret)
             VALUES ($1, $2, $3, 30, '')
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(name)
        .bind(created_at)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn import_table_pilots(sqlite: &Connection, pool: &PgPool) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare(
            "SELECT id, club_id, name, email, license_no, status, role, password_hash, created_at
             FROM pilots",
        )
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })
        .map_err(sqlite_err)?;
    for row in rows {
        let (id, club_id, name, email, license_no, status, role, hash, created_at) =
            row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(club_id)
        .bind(name)
        .bind(email)
        .bind(license_no)
        .bind(status)
        .bind(role)
        .bind(hash)
        .bind(created_at)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn import_table_aircraft_types(
    sqlite: &Connection,
    pool: &PgPool,
) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare("SELECT id, club_id, code FROM aircraft_types")
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(sqlite_err)?;
    for row in rows {
        let (id, club_id, code) = row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO aircraft_types (id, club_id, code) VALUES ($1, $2, $3)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(club_id)
        .bind(code)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn import_table_aircraft(sqlite: &Connection, pool: &PgPool) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare("SELECT id, club_id, tail, type_id, active FROM aircraft")
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)? != 0,
            ))
        })
        .map_err(sqlite_err)?;
    for row in rows {
        let (id, club_id, tail, type_id, active) = row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(club_id)
        .bind(tail)
        .bind(type_id)
        .bind(active)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn import_table_devices(sqlite: &Connection, pool: &PgPool) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare("SELECT id, pilot_id, token_hash, label, created_at FROM devices")
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(sqlite_err)?;
    for row in rows {
        let (id, pilot_id, token_hash, label, created_at) = row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO devices (id, pilot_id, token_hash, label, created_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(pilot_id)
        .bind(token_hash)
        .bind(label)
        .bind(created_at)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn import_table_device_pair_tokens(
    sqlite: &Connection,
    pool: &PgPool,
) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare("SELECT device_id, token FROM device_pair_tokens")
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(sqlite_err)?;
    for row in rows {
        let (device_id, token) = row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO device_pair_tokens (device_id, token) VALUES ($1, $2)
             ON CONFLICT (device_id) DO NOTHING",
        )
        .bind(device_id)
        .bind(token)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn import_table_flights(sqlite: &Connection, pool: &PgPool) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare(
            "SELECT id, pilot_id, aircraft_id, status, started_at, ended_at, last_fix_at,
             visibility, show_pilot_name_public FROM flights",
        )
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)? != 0,
            ))
        })
        .map_err(sqlite_err)?;
    for row in rows {
        let (
            id,
            pilot_id,
            aircraft_id,
            status,
            started_at,
            ended_at,
            last_fix_at,
            visibility,
            show_public,
        ) = row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO flights (id, pilot_id, aircraft_id, status, started_at, ended_at, last_fix_at, visibility, show_pilot_name_public)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(pilot_id)
        .bind(aircraft_id)
        .bind(status)
        .bind(started_at)
        .bind(ended_at)
        .bind(last_fix_at)
        .bind(visibility)
        .bind(show_public)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn import_table_fixes(sqlite: &Connection, pool: &PgPool) -> Result<(), DbConnectError> {
    let mut stmt = sqlite
        .prepare(
            "SELECT flight_id, device_id, t_ms, lat, lon, alt, speed, course, accuracy FROM fixes",
        )
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, Option<f64>>(6)?,
                r.get::<_, Option<f64>>(7)?,
                r.get::<_, Option<f64>>(8)?,
            ))
        })
        .map_err(sqlite_err)?;
    let now = crate::club::now_ms();
    for row in rows {
        let (flight_id, device_id, t_ms, lat, lon, alt, speed, course, accuracy) =
            row.map_err(sqlite_err)?;
        sqlx::query(
            "INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, speed, course, accuracy)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (flight_id, device_id, t_ms) DO NOTHING",
        )
        .bind(flight_id)
        .bind(device_id)
        .bind(t_ms)
        .bind(now)
        .bind(lat)
        .bind(lon)
        .bind(alt)
        .bind(speed)
        .bind(course)
        .bind(accuracy)
        .execute(pool)
        .await?;
    }
    Ok(())
}
