use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::{env, path::Path, time::Duration};

#[derive(Debug)]
pub enum DbConnectError {
    MissingUrl,
    ReadUrlFile(std::io::Error),
    Sqlx(sqlx::Error),
}

impl std::fmt::Display for DbConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingUrl => write!(f, "DATABASE_URL or DATABASE_URL_FILE required"),
            Self::ReadUrlFile(e) => write!(f, "read DATABASE_URL_FILE: {e}"),
            Self::Sqlx(e) => write!(f, "{e}"),
        }
    }
}

impl From<sqlx::Error> for DbConnectError {
    fn from(e: sqlx::Error) -> Self {
        Self::Sqlx(e)
    }
}

pub fn resolve_database_url() -> Result<String, DbConnectError> {
    if let Ok(url) = env::var("DATABASE_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(path) = env::var("DATABASE_URL_FILE") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            let bytes = std::fs::read_to_string(trimmed).map_err(DbConnectError::ReadUrlFile)?;
            let url = bytes.trim();
            if url.is_empty() {
                return Err(DbConnectError::MissingUrl);
            }
            return Ok(url.to_string());
        }
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

pub async fn run_migrations(pool: &PgPool) -> Result<(), DbConnectError> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(DbConnectError::from)
}

/// Postgres for integration tests (CI sets DATABASE_URL).
pub async fn open_test_pool() -> Result<PgPool, DbConnectError> {
    connect_pool().await
}

pub fn legacy_sqlite_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("club.sqlite")
}