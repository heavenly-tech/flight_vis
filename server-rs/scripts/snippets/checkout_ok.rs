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