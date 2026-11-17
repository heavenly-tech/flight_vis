#!/usr/bin/env python3
from pathlib import Path

DB = Path(__file__).resolve().parents[1] / "src/club/db_pg.rs"
text = DB.read_text()
start = text.index("    async fn checkout_ok(")
end = text.index("    pub async fn me_tails(", start)
block = '''    async fn checkout_ok(
        pool: &PgPool,
        pilot_id: &str,
        aircraft_id: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        let row = sqlx::query("SELECT type_id, active FROM aircraft WHERE id = $1")
            .bind(aircraft_id)
            .fetch_one(pool)
            .await?;
        let type_id: String = row.try_get(0)?;
        let active: bool = row.try_get(1)?;
        if !active {
            return Ok(false);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM checkouts c
             WHERE c.pilot_id = $1
               AND (c.expires_at IS NULL OR c.expires_at > $2)
               AND ((c.aircraft_id = $3) OR (c.aircraft_id IS NULL AND c.type_id = $4))",
        )
        .bind(pilot_id)
        .bind(now)
        .bind(aircraft_id)
        .bind(&type_id)
        .fetch_one(pool)
        .await?;
        Ok(count > 0)
    }

'''
DB.write_text(text[:start] + block + text[end:])
print("patched checkout_ok")