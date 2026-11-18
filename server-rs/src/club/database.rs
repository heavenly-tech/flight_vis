use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::{env, path::Path, time::Duration};

/// Minimum Postgres major version supported in production and CI (no extensions required).
#[allow(dead_code)]
pub const MIN_POSTGRES_MAJOR: u32 = 18;

#[derive(Debug)]
pub enum DbConnectError {
    MissingUrl,
    ReadUrlFile(std::io::Error),
    ReadPasswordFile(std::io::Error),
    Sqlx(sqlx::Error),
}

impl std::fmt::Display for DbConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingUrl => write!(
                f,
                "set DATABASE_URL, DATABASE_URL_FILE, or PGHOST/PGPORT/PGDATABASE/PGUSER with DATABASE_PASSWORD_FILE or PGPASSWORD"
            ),
            Self::ReadUrlFile(e) => write!(f, "read DATABASE_URL_FILE: {e}"),
            Self::ReadPasswordFile(e) => write!(f, "read DATABASE_PASSWORD_FILE: {e}"),
            Self::Sqlx(e) => write!(f, "{e}"),
        }
    }
}

impl From<sqlx::Error> for DbConnectError {
    fn from(e: sqlx::Error) -> Self {
        Self::Sqlx(e)
    }
}

fn read_trimmed_file(path: &str) -> Result<String, std::io::Error> {
    let bytes = std::fs::read_to_string(path)?;
    Ok(bytes.trim().to_string())
}

fn pg_password() -> Result<Option<String>, DbConnectError> {
    if let Ok(path) = env::var("DATABASE_PASSWORD_FILE") {
        let path = path.trim();
        if !path.is_empty() {
            let pw = read_trimmed_file(path).map_err(DbConnectError::ReadPasswordFile)?;
            if pw.is_empty() {
                return Ok(None);
            }
            return Ok(Some(pw));
        }
    }
    if let Ok(pw) = env::var("PGPASSWORD") {
        let pw = pw.trim();
        if !pw.is_empty() {
            return Ok(Some(pw.to_string()));
        }
    }
    Ok(None)
}

/// Percent-encode userinfo for a postgres URL (minimal set).
fn urlencode_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build `postgres://user:pass@host:port/db` from heavenly-style env (password file supported).
pub fn database_url_from_pg_env() -> Result<Option<String>, DbConnectError> {
    let host = match env::var("PGHOST") {
        Ok(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => return Ok(None),
    };
    let port = env::var("PGPORT")
        .ok()
        .and_then(|p| {
            if p.trim().is_empty() {
                None
            } else {
                Some(p.trim().to_string())
            }
        })
        .unwrap_or_else(|| "5432".to_string());
    let dbname = match env::var("PGDATABASE") {
        Ok(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => return Ok(None),
    };
    let user = match env::var("PGUSER") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => return Ok(None),
    };
    let password = pg_password()?.unwrap_or_default();
    let user_enc = urlencode_userinfo(&user);
    let url = if password.is_empty() {
        format!("postgres://{user_enc}@{host}:{port}/{dbname}")
    } else {
        let pass_enc = urlencode_userinfo(&password);
        format!("postgres://{user_enc}:{pass_enc}@{host}:{port}/{dbname}")
    };
    Ok(Some(url))
}

#[cfg(test)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn resolve_database_url() -> Result<String, DbConnectError> {
    #[cfg(test)]
    let _env_guard = env_lock();
    if let Ok(url) = env::var("DATABASE_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(path) = env::var("DATABASE_URL_FILE") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            let url = read_trimmed_file(trimmed).map_err(DbConnectError::ReadUrlFile)?;
            if url.is_empty() {
                return Err(DbConnectError::MissingUrl);
            }
            return Ok(url);
        }
    }
    if let Some(url) = database_url_from_pg_env()? {
        return Ok(url);
    }
    Err(DbConnectError::MissingUrl)
}

pub async fn connect_pool() -> Result<PgPool, DbConnectError> {
    let url = resolve_database_url()?;
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await?;
    Ok(pool)
}

#[cfg(test)]
static TEST_MIGRATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One shared Postgres database in CI; hold this for the whole `ClubDb::open_test` lifetime.
#[cfg(test)]
static TEST_PG_INTEGRATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
#[allow(dead_code)]
pub(crate) async fn acquire_test_pg_integration_lock() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_PG_INTEGRATION_LOCK.lock().await
}

#[allow(dead_code)]
pub async fn run_migrations(pool: &PgPool) -> Result<(), DbConnectError> {
    #[cfg(test)]
    let _migrate_guard = TEST_MIGRATE_LOCK.lock().await;
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| DbConnectError::Sqlx(sqlx::Error::Migrate(Box::new(e))))
}

/// Postgres for integration tests (CI sets DATABASE_URL).
#[cfg(test)]
#[allow(dead_code)]
pub async fn open_test_pool() -> Result<PgPool, DbConnectError> {
    connect_pool().await
}

#[allow(dead_code)]
pub fn legacy_sqlite_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("club.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_url_from_pg_env_and_password_file() {
        let _g = env_lock();
        const KEYS: [&str; 8] = [
            "DATABASE_URL",
            "DATABASE_URL_FILE",
            "PGHOST",
            "PGPORT",
            "PGDATABASE",
            "PGUSER",
            "PGPASSWORD",
            "DATABASE_PASSWORD_FILE",
        ];
        let saved: Vec<(String, Option<String>)> = KEYS
            .iter()
            .map(|k| ((*k).to_string(), env::var(k).ok()))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let pw_path = dir.path().join("pgpass");
        std::fs::write(&pw_path, "s3cret!\n").unwrap();
        for key in KEYS {
            env::remove_var(key);
        }
        env::set_var("PGHOST", "db.heavenly.test");
        env::set_var("PGPORT", "5433");
        env::set_var("PGDATABASE", "flight_vis");
        env::set_var("PGUSER", "flight_vis");
        env::set_var("DATABASE_PASSWORD_FILE", pw_path.to_string_lossy().as_ref());
        let url = resolve_database_url().expect("url");
        assert!(url.contains("db.heavenly.test:5433/flight_vis"));
        assert!(url.contains("flight_vis"));
        assert!(url.contains("s3cret%21"));
        for (key, val) in saved {
            restore_var(&key, val);
        }
    }

    #[test]
    fn database_url_takes_precedence() {
        let _g = env_lock();
        let saved_url = env::var("DATABASE_URL").ok();
        let saved_pghost = env::var("PGHOST").ok();
        env::set_var("DATABASE_URL", "postgres://u:p@h/d");
        env::set_var("PGHOST", "ignored");
        let url = resolve_database_url().unwrap();
        assert_eq!(url, "postgres://u:p@h/d");
        restore_var("DATABASE_URL", saved_url);
        restore_var("PGHOST", saved_pghost);
    }

    fn restore_var(key: &str, value: Option<String>) {
        if let Some(v) = value {
            env::set_var(key, v);
        } else {
            env::remove_var(key);
        }
    }

    /// CI runs this with `DATABASE_URL=postgres://flightvis:…/flight_vis_owner_test` after creating the NOSUPERUSER role.
    #[tokio::test]
    #[serial_test::serial]
    async fn flightvis_owner_migrations() {
        let _db_guard = acquire_test_pg_integration_lock().await;
        let pool = connect_pool()
            .await
            .expect("database url for owner smoke test");
        run_migrations(&pool)
            .await
            .expect("migrations as flightvis owner");
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'pilots'
             )",
        )
        .fetch_one(&pool)
        .await
        .expect("query");
        assert!(exists);
        pool.close().await;
    }
}
