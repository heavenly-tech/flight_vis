//! Jordan / Kepler follow-up security tests (PR #24).

use crate::club::auth::{hash_password, new_id};
use crate::club::db::LoginError;
use crate::club::{db::now_ms, ClubDb};
use serial_test::serial;
use sqlx::Row;
use std::collections::HashSet;

#[tokio::test]
#[serial]
async fn first_magic_verify_wipes_precreated_password_and_sessions() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "verify-wipe@test";
    let hash = hash_password("precreated12").unwrap();
    let id = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, created_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&id)
    .bind(email)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sess = db.create_account_session(&id, None, now).await.unwrap();
    let raw = db.issue_account_magic_login(email, now).await.unwrap().unwrap();
    let _ = db
        .consume_account_magic(&raw, "account_login", now)
        .await
        .unwrap();
    let hash_after: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
            .bind(&id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(hash_after.as_deref().unwrap_or("").is_empty());
    assert!(db.account_from_session(&sess, now).await.is_none());
}

#[tokio::test]
#[serial]
async fn superadmin_account_global_role_requires_verified_approved_pilot() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "sa-gated@test";
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, global_role, created_at, email_verified_at)
         VALUES ($1, $2, 'superadmin', $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot_pending = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'P', $4, '', 'pending', 'pilot', '', $5)",
    )
    .bind(&pilot_pending)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let allow = HashSet::new();
    assert!(!db.is_platform_superadmin(&pilot_pending, &allow).await);
    sqlx::query(
        "UPDATE pilots SET status = 'approved', email_verified_at = $1 WHERE id = $2",
    )
    .bind(now)
    .bind(&pilot_pending)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db.is_platform_superadmin(&pilot_pending, &allow).await);
}

#[tokio::test]
#[serial]
async fn approve_membership_rejects_non_pending() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'R', 'removed-admin@test', '', 'approved', 'admin', 'x', $3)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'admin', 'removed', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db.approve_membership(&mid, now).await.is_err());
}

#[tokio::test]
#[serial]
async fn suspended_account_blocks_legacy_pilot_password_login() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let hash = hash_password("password12").unwrap();
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, created_at, email_verified_at, suspended_at)
         VALUES ($1, 'legacy-sus@test', $2, $3, $3, $3)",
    )
    .bind(&acct)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, 'L', 'legacy-sus@test', '', 'approved', 'pilot', $4, $5, $5)",
    )
    .bind(new_id())
    .bind(&club_id)
    .bind(&acct)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pid: String = sqlx::query_scalar("SELECT id FROM pilots WHERE account_id = $1")
        .bind(&acct)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let row = sqlx::query("SELECT club_id FROM pilots WHERE id = $1")
        .bind(&pid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let cid: String = row.try_get(0).unwrap();
    assert!(matches!(
        db.login_password(&cid, "legacy-sus@test", "password12", now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn pilot_session_for_account_reuses_one_row_per_account_session() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "reuse-ps@test";
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at)
         VALUES ($1, $2, $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'R', $4, '', 'approved', 'pilot', '', $5)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let acct_sess = db
        .create_account_session(&acct, Some(&mid), now)
        .await
        .unwrap();
  let count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM pilot_sessions WHERE pilot_id = $1")
            .bind(&pilot_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let (s1, _) = db
        .pilot_session_for_account(&acct_sess, now)
        .await
        .unwrap();
    let (s2, _) = db
        .pilot_session_for_account(&acct_sess, now)
        .await
        .unwrap();
    assert_eq!(s1, s2);
    let count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM pilot_sessions WHERE pilot_id = $1")
            .bind(&pilot_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count_after, count_before + 1);
}

#[tokio::test]
#[serial]
async fn admin_invite_ignores_pending_lookalike_pilot() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "lookalike-admin@test";
    let attacker_pilot = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Evil', $3, '', 'pending', 'admin', 'x', $4)",
    )
    .bind(&attacker_pilot)
    .bind(&club_id)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let inviter = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Inv', 'inv-lookalike@test', '', 'approved', 'admin', 'x', $3)",
    )
    .bind(&inviter)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let raw = db
        .invite_admin_membership(&club_id, email, &inviter, now)
        .await
        .unwrap();
    let accept = db.accept_admin_invite(&raw, now).await.unwrap();
    let promoted: String =
        sqlx::query_scalar("SELECT pilot_id FROM club_memberships WHERE id = $1")
            .bind(&accept.membership_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_ne!(promoted, attacker_pilot);
    let attacker_gone: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM pilots WHERE id = $1)",
    )
    .bind(&attacker_pilot)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(attacker_gone);
}