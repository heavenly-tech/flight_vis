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

    pub async fn set_account_locale(&self, account_id: &str, locale: &str) -> Result<(), DbError> {
        if !Self::locale_known(locale) {
            return Err(DbError::protocol("locale"));
        }
        sqlx::query("UPDATE accounts SET locale = $1 WHERE id = $2")
            .bind(locale)
            .bind(account_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn club_id_for_membership(&self, membership_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT club_id FROM club_memberships WHERE id = $1")
            .bind(membership_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn user_club_locale_context(
        &self,
        headers: &axum::http::HeaderMap,
        now: i64,
    ) -> (Option<String>, Option<String>) {
        use crate::club::auth::{self, parse_session_cookie};
        if let Some(acct_sess) = auth::parse_account_session_cookie(headers) {
            if let Some((account, active_mem)) = self.account_from_session(&acct_sess, now).await {
                let club_default = if let Some(mem_id) = active_mem.as_ref() {
                    if let Some(cid) = self.club_id_for_membership(mem_id).await {
                        self.club_default_locale(&cid).await.ok()
                    } else {
                        None
                    }
                } else {
                    None
                };
                return (account.locale, club_default);
            }
        }
        let session = parse_session_cookie(headers);
        let (_viewer, pilot) = self.viewer_with_pilot(session.as_deref(), now).await;
        match pilot {
            Some(p) => (
                p.locale.clone(),
                self.club_default_locale(&p.club_id)
                    .await
                    .ok()
                    .or(Some("es".to_string())),
            ),
            None => (None, None),
        }
    }

    pub async fn club_default_locale(&self, club_id: &str) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT default_locale FROM clubs WHERE id = $1")
            .bind(club_id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn set_club_default_locale(
        &self,
        club_id: &str,
        locale: &str,
    ) -> Result<(), DbError> {
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
