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
async fn club_creator_signs_in_via_account_after_verify() {
    let db = open_db().await;
    let now = now_ms();
    let slug = format!("creator-{}", &crate::club::auth::new_id()[..8]);
    let email = format!("owner-{slug}@test");
    let (club_id, token) = db
        .signup_club("Creator Club", &slug, &email, None, now)
        .await
        .expect("signup");
    db.try_consume_magic_club_verify(&token, now)
        .await
        .expect("verify");
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "UPDATE accounts SET password_hash = $1 WHERE lower(trim(email)) = lower(trim($2))",
    )
    .bind(&hash)
    .bind(&email)
    .execute(&db.pool)
    .await
    .unwrap();
    let (session_id, account) = db
        .account_login_password(&email, "password12", now, "127.0.0.1")
        .await
        .expect("account login after verify");
    let memberships = db.list_account_memberships(&account.id).await;
    let admin = memberships
        .iter()
        .find(|m| m.club_id == club_id && m.role == "admin" && m.status == "active")
        .expect("active admin membership on new club, not default_club");
    let pilot = db
        .set_active_membership(&session_id, &admin.id, now)
        .await
        .expect("switch into verified club");
    assert_eq!(pilot.club_id, club_id);
    assert!(db.require_club_admin(&pilot.id, &club_id).await.is_ok());
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

#[serial]
#[tokio::test]
async fn account_switch_rejects_suspended_club() {
    let db = open_db().await;
    let now = now_ms();
    let slug = format!("sus-sw-{}", &crate::club::auth::new_id()[..8]);
    let (club_id, token) = db
        .signup_club("Sus Switch", &slug, "sus-sw@test", None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&token, now).await.unwrap();
    let acct = db
        .create_account("sus-sw@test", Some("password12"), now)
        .await
        .unwrap();
    let mid: String = sqlx::query_scalar(
        "SELECT m.id FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE p.account_id = $1 AND m.club_id = $2",
    )
    .bind(&acct)
    .bind(&club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let (sess, _) = db
        .account_login_password("sus-sw@test", "password12", now, "127.0.0.1")
        .await
        .unwrap();
    db.set_club_suspended(&club_id, now).await.unwrap();
    assert!(db.set_active_membership(&sess, &mid, now).await.is_err());
}

#[serial]
#[tokio::test]
async fn aircraft_tail_lookup_scoped_by_club() {
    let db = open_db().await;
    let now = now_ms();
    let club_a = db.default_club_id().await.unwrap();
    let slug = format!("tail-b-{}", &crate::club::auth::new_id()[..8]);
    let (club_b, tok) = db
        .signup_club("Tail B", &slug, "tail-b@test", None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&tok, now).await.unwrap();
    let tail = "DUP-123";
    sqlx::query(
        "INSERT INTO aircraft (id, club_id, tail, type_code, active, created_at)
         VALUES ($1, $2, $3, 'GLID', true, $4), ($5, $6, $3, 'GLID', true, $4)",
    )
    .bind(crate::club::auth::new_id())
    .bind(&club_a)
    .bind(tail)
    .bind(now)
    .bind(crate::club::auth::new_id())
    .bind(&club_b)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        db.club_id_for_aircraft_tail(&club_a, tail).await.as_deref(),
        Some(club_a.as_str())
    );
    assert_eq!(
        db.club_id_for_aircraft_tail(&club_b, tail).await.as_deref(),
        Some(club_b.as_str())
    );
    assert!(db
        .club_id_for_aircraft_tail(&club_a, "NOPE")
        .await
        .is_none());
}
