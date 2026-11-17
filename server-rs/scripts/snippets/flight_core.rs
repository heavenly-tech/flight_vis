    pub async fn approve_pilot(&self, pilot_id: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE pilots SET status = 'approved' WHERE id = $1")
            .bind(pilot_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE club_memberships SET status = 'approved' WHERE pilot_id = $1")
            .bind(pilot_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub(crate) async fn checkout_ok(
        pool: &PgPool,
        pilot_id: &str,
        aircraft_id: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        let row = sqlx::query("SELECT type_id, active FROM aircraft WHERE id = $1")
            .bind(aircraft_id)
            .fetch_optional(pool)
            .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let type_id: String = row.try_get(0)?;
        let active: bool = row.try_get(1)?;
        if !active {
            return Ok(false);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM checkouts c
             WHERE c.pilot_id = $1
               AND (c.expires_at IS NULL OR c.expires_at > $2)
               AND (
                 (c.aircraft_id = $3) OR
                 (c.aircraft_id IS NULL AND c.type_id = $4)
               )",
        )
        .bind(pilot_id)
        .bind(now)
        .bind(aircraft_id)
        .bind(type_id)
        .fetch_one(pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn me_tails(&self, pilot_id: &str, now: i64) -> Result<Vec<TailInfo>, DbError> {
        let rows = sqlx::query(
            "SELECT a.id, a.tail, t.code
             FROM aircraft a
             JOIN aircraft_types t ON t.id = a.type_id
             WHERE a.active = true",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get(0)?;
            let tail: String = row.try_get(1)?;
            let type_code: String = row.try_get(2)?;
            if Self::checkout_ok(&self.pool, pilot_id, &id, now).await? {
                out.push(TailInfo {
                    id,
                    tail,
                    type_code,
                });
            }
        }
        Ok(out)
    }