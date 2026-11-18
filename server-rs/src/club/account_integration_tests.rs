//! Integration tests for accounts, memberships, and legacy compat.

use crate::club::{
    auth::hash_password,
    db::{now_ms, ClubDb},
};
use serial_test::serial;

async fn open_db() -> ClubDb {
    ClubDb::open_test().await.expect("db")
}

#[serial]
#[tokio::test]
async fn legacy_pilot_session_login_still_works() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    let hash = hash_password("password12").unwrap();
    let pid = crate::club::auth::new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Legacy', 'legacy@test', '', 'approved', 'pilot', $3, $4)",
    )
    .bind(&pid)
    .bind(&club)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let _ = db
        .ensure_membership_row(&pid, &club, "pilot", "active")
        .await;
    let (sess, pilot) = db
        .login_password(&club, "legacy@test", "password12", now, "127.0.0.1")
        .await
        .unwrap();
    assert_eq!(pilot.id, pid);
    assert!(!sess.is_empty());
}

#[serial]
#[tokio::test]
async fn pending_membership_denies_login() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    let hash = hash_password("password12").unwrap();
    let pid = crate::club::auth::new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Pend', 'pend@test', '', 'pending', 'pilot', $3, $4)",
    )
    .bind(&pid)
    .bind(&club)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pid, &club, "pilot", "pending")
        .await
        .unwrap();
    assert!(db
        .login_password(&club, "pend@test", "password12", now, "127.0.0.1")
        .await
        .is_err());
}

#[serial]
#[tokio::test]
async fn account_switch_active_club() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    let email = "switch@test";
    let hash = hash_password("password12").unwrap();
    let now = now_ms();
    let acct = db
        .create_account(email, Some("password12"), now)
        .await
        .unwrap();
    let pid = crate::club::auth::new_id();
    let mid = crate::club::auth::new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'Sw', $4, '', 'approved', 'pilot', $5, $6)",
    )
    .bind(&pid)
    .bind(&club)
    .bind(&acct)
    .bind(email)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pid)
    .bind(&club)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let (acct_sess, _) = db
        .account_login_password(email, "password12", now, "127.0.0.1")
        .await
        .unwrap();
    let pilot = db
        .set_active_membership(&acct_sess, &mid, now)
        .await
        .unwrap();
    assert_eq!(pilot.club_id, club);
}

#[serial]
#[tokio::test]
async fn join_then_approve_membership() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    let now = now_ms();
    sqlx::query("UPDATE clubs SET status = 'live', email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&club)
        .execute(&db.pool)
        .await
        .unwrap();
    let acct = db
        .create_account("joiner@test", Some("password12"), now)
        .await
        .unwrap();
    let mid = db
        .join_club_with_account(&acct, &club, "Joiner", "", now)
        .await
        .unwrap();
    db.approve_membership(&mid, now).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM club_memberships WHERE id = $1")
        .bind(&mid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
}

#[serial]
#[tokio::test]
async fn suspended_account_cannot_login() {
    let db = open_db().await;
    let now = now_ms();
    let acct = db
        .create_account("sus@test", Some("password12"), now)
        .await
        .unwrap();
    db.set_account_suspended(&acct, now).await.unwrap();
    assert!(db
        .account_login_password("sus@test", "password12", now, "127.0.0.1")
        .await
        .is_err());
}
