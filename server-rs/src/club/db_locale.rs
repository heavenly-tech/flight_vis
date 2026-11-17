use crate::club::db::{ClubDb, DbError};

impl ClubDb {
    fn locale_known(locale: &str) -> bool {
        crate::i18n::global().is_some_and(|c| c.is_known(locale))
    }

    pub async fn set_pilot_locale(&self, pilot_id: &str, locale: &str) -> Result<(), DbError> {
        if !Self::locale_known(locale) {
            return Err(DbError::protocol("locale"));
        }
        sqlx::query("UPDATE pilots SET locale = $1 WHERE id = $2")
            .bind(locale)
            .bind(pilot_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn club_default_locale(&self, club_id: &str) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT default_locale FROM clubs WHERE id = $1")
            .bind(club_id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn set_club_default_locale(&self, club_id: &str, locale: &str) -> Result<(), DbError> {
        if !Self::locale_known(locale) {
            return Err(DbError::protocol("locale"));
        }
        sqlx::query("UPDATE clubs SET default_locale = $1 WHERE id = $2")
            .bind(locale)
            .bind(club_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}