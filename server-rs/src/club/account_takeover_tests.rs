//! C1: account signup / password must not hijack magic-only or invited emails.

use crate::club::auth::hash_password;
use crate::club::db::LoginError;
use crate::club::{auth, auth::new_id, db::now_ms, ClubDb};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn signup_on_existing_magic_only_does_not_set_password() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "magic-only@test";
  let id = db.ensure_account_for_email(email, None, now).await.unwrap();
    let hash_before: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
            .bind(&id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(hash_before.as_deref().unwrap_or("").is_empty());
    let _ = db
        .ensure_account_for_email(email, Some("password1234"), now)
        .await
        .unwrap();
    let hash_after: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
            .bind(&id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(hash_after.as_deref().unwrap_or("").is_empty());
}

#[tokio::test]
#[serial]
async fn unverified_account_cannot_password_login_or_switch_club() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "unverified-login@test";
    let hash = hash_password("password12").unwrap();
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
    assert!(matches!(
        db.account_login_password(email, "password12", now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
    let club_id = db.default_club_id().await.unwrap();
    let pilot_id = auth::new_id();
    let member_id = auth::new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'U', $4, '', 'approved', 'pilot', '', $5)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&id)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'active', $4)",
    )
    .bind(&member_id)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sess = db.create_account_session(&id, None, now).await.unwrap();
    assert!(db
        .set_active_membership(&sess, &member_id, now)
        .await
        .is_err());
}

#[tokio::test]
#[serial]
async fn admin_invite_clears_hijacker_password_on_unverified_account() {
    use crate::club::db_memberships_admin::ADMIN_INVITE_PURPOSE;
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "invite-hijack@test";
    let _ = db
        .ensure_account_for_email(email, Some("hijackpw12"), now)
        .await
        .unwrap();
    let acct_id: String = sqlx::query_scalar("SELECT id FROM accounts WHERE lower(email) = lower($1)")
        .bind(email)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let inviter = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Inviter', 'inviter-hijack@test', '', 'approved', 'admin', 'x', $3)",
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
    assert_eq!(accept.club_id, club_id);
    assert!(matches!(
        db.account_login_password(email, "hijackpw12", now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
    let hash_after: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
            .bind(&acct_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(hash_after.as_deref().unwrap_or("").is_empty());
    let verified: Option<i64> =
        sqlx::query_scalar("SELECT email_verified_at FROM accounts WHERE id = $1")
            .bind(&acct_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(verified.is_some());
    let _ = ADMIN_INVITE_PURPOSE;
}