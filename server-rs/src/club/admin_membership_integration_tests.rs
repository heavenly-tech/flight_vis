//! Multi-admin membership integration tests (serial DB).

use crate::club::{
    auth::hash_password,
    db::{now_ms, ClubDb},
    db_memberships_admin::{MembershipAdminError, ADMIN_INVITE_PURPOSE, PILOT_INVITE_PURPOSE},
};
use serial_test::serial;
use sqlx::Row;

async fn open_db() -> ClubDb {
    ClubDb::open_test().await.expect("db")
}

async fn ensure_live_club(db: &ClubDb, club_id: &str) {
    let now = now_ms();
    let _ = sqlx::query(
        "UPDATE clubs SET email_verified_at = COALESCE(email_verified_at, $1), status = 'live' WHERE id = $2",
    )
    .bind(now)
    .bind(club_id)
    .execute(&db.pool)
    .await;
}

async fn insert_admin(db: &ClubDb, club_id: &str, email: &str, name: &str) -> (String, String) {
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = crate::club::auth::new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, $4, '', 'approved', 'admin', $5, $6)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(name)
    .bind(email)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pilot_id, club_id, "admin", "active")
        .await
        .unwrap();
    let mid: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pilot_id)
            .bind(club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    (pilot_id, mid)
}

async fn insert_staff(db: &ClubDb, club_id: &str, email: &str) -> (String, String) {
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = crate::club::auth::new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Staff', $3, '', 'approved', 'staff', $4, $5)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(email)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pilot_id, club_id, "staff", "active")
        .await
        .unwrap();
    let mid: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pilot_id)
            .bind(club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    (pilot_id, mid)
}

#[serial]
#[tokio::test]
async fn admin_invite_and_accept_creates_active_admin() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let (inviter, _) = insert_admin(&db, &club, "inviter@test", "Inv").await;
    let now = now_ms();
    let raw = db
        .invite_admin_membership(&club, "newadmin@test", &inviter, now)
        .await
        .unwrap();
    let accept = db.accept_admin_invite(&raw, now).await.unwrap();
    assert_eq!(accept.club_id, club);
    let row = sqlx::query("SELECT role, status FROM club_memberships WHERE id = $1")
        .bind(&accept.membership_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let role: String = row.try_get(0).unwrap();
    let status: String = row.try_get(1).unwrap();
    assert_eq!(role, "admin");
    assert_eq!(status, "active");
    let purpose: String =
        sqlx::query_scalar("SELECT purpose FROM magic_login_tokens WHERE purpose = $1 LIMIT 1")
            .bind(ADMIN_INVITE_PURPOSE)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(purpose, ADMIN_INVITE_PURPOSE);
}

#[serial]
#[tokio::test]
async fn admin_promote_staff_to_admin() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let _ = insert_admin(&db, &club, "boss@test", "Boss").await;
    let (_, staff_mid) = insert_staff(&db, &club, "staff@test").await;
    db.set_membership_role(&club, &staff_mid, "admin")
        .await
        .unwrap();
    let role: String = sqlx::query_scalar("SELECT role FROM club_memberships WHERE id = $1")
        .bind(&staff_mid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(role, "admin");
}

#[serial]
#[tokio::test]
async fn admin_demote_admin_to_staff() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let _ = insert_admin(&db, &club, "a1@test", "A1").await;
    let (_, mid2) = insert_admin(&db, &club, "a2@test", "A2").await;
    db.set_membership_role(&club, &mid2, "staff").await.unwrap();
    let role: String = sqlx::query_scalar("SELECT role FROM club_memberships WHERE id = $1")
        .bind(&mid2)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(role, "staff");
}

#[serial]
#[tokio::test]
async fn admin_remove_admin_membership() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let _ = insert_admin(&db, &club, "keep@test", "Keep").await;
    let (_, rem) = insert_admin(&db, &club, "gone@test", "Gone").await;
    db.remove_admin_membership(&club, &rem).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM club_memberships WHERE id = $1")
        .bind(&rem)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "suspended");
}

#[serial]
#[tokio::test]
async fn last_admin_remove_blocked() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let (_, only) = insert_admin(&db, &club, "solo@test", "Solo").await;
    assert_eq!(
        db.remove_admin_membership(&club, &only).await,
        Err(MembershipAdminError::LastAdmin)
    );
}

#[serial]
#[tokio::test]
async fn last_admin_self_demote_blocked() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let (_, only) = insert_admin(&db, &club, "solo2@test", "Solo2").await;
    assert_eq!(
        db.set_membership_role(&club, &only, "staff").await,
        Err(MembershipAdminError::LastAdmin)
    );
}

#[serial]
#[tokio::test]
async fn concurrent_double_demote_two_admins_leaves_one() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let (_, mid_a) = insert_admin(&db, &club, "cda@test", "A").await;
    let (_, mid_b) = insert_admin(&db, &club, "cdb@test", "B").await;
    let db_a = db.clone();
    let db_b = db.clone();
    let club_a = club.clone();
    let club_b = club.clone();
    let ma = mid_a.clone();
    let mb = mid_b.clone();
    let (r1, r2) = tokio::join!(
        async move { db_a.set_membership_role(&club_a, &ma, "staff").await },
        async move { db_b.set_membership_role(&club_b, &mb, "staff").await },
    );
    let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
    let last_admin_count = [&r1, &r2]
        .iter()
        .filter(|r| matches!(r, Err(MembershipAdminError::LastAdmin)))
        .count();
    assert_eq!(ok_count, 1);
    assert_eq!(last_admin_count, 1);
    let admins: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM club_memberships
         WHERE club_id = $1 AND role = 'admin' AND status = 'active'",
    )
    .bind(&club)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(admins, 1);
}

#[serial]
#[tokio::test]
async fn non_admin_invite_promote_403() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let (pilot_id, _) = {
        let now = now_ms();
        let hash = hash_password("password12").unwrap();
        let pilot_id = crate::club::auth::new_id();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, 'P', 'pilotonly@test', '', 'approved', 'pilot', $3, $4)",
        )
        .bind(&pilot_id)
        .bind(&club)
        .bind(&hash)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        db.ensure_membership_row(&pilot_id, &club, "pilot", "active")
            .await
            .unwrap();
        let mid: String = sqlx::query_scalar(
            "SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2",
        )
        .bind(&pilot_id)
        .bind(&club)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        (pilot_id, mid)
    };
    assert!(db
        .require_active_club_admin_role(&pilot_id, &club)
        .await
        .is_err());
    let (_, staff_mid) = insert_staff(&db, &club, "staff403@test").await;
    assert!(db
        .require_active_club_admin_role(
            &sqlx::query_scalar::<_, String>("SELECT pilot_id FROM club_memberships WHERE id = $1")
                .bind(&staff_mid)
                .fetch_one(&db.pool)
                .await
                .unwrap(),
            &club
        )
        .await
        .is_err());
}

#[serial]
#[tokio::test]
async fn pilot_invite_and_accept_creates_active_pilot_and_audit() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let (inviter, _) = insert_admin(&db, &club, "inviter-pilot@test", "Inv").await;
    let now = now_ms();
    let raw = db
        .invite_membership(&club, "newpilot@test", &inviter, "pilot", now)
        .await
        .unwrap();
    assert_eq!(
        db.peek_magic_token_purpose(&raw).await.as_deref(),
        Some(PILOT_INVITE_PURPOSE)
    );
    let token_id: String = sqlx::query_scalar(
        "SELECT id FROM magic_login_tokens WHERE lower(email) = 'newpilot@test' AND purpose = $1",
    )
    .bind(PILOT_INVITE_PURPOSE)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let invite_audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM audit_log
         WHERE club_id = $1 AND actor_pilot_id = $2 AND action = 'invite_pilot'
           AND entity_type = 'membership' AND entity_id = $3 AND detail = 'pilot'",
    )
    .bind(&club)
    .bind(&inviter)
    .bind(&token_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(invite_audit_count, 1);

    // Even if invoked through the legacy accept_admin_invite wrapper, role stays pilot.
    let accept = db.accept_admin_invite(&raw, now).await.unwrap();
    assert_eq!(accept.club_id, club);
    assert_eq!(accept.purpose, PILOT_INVITE_PURPOSE);
    assert_eq!(accept.role, "pilot");
    let (m_role, m_status, pid, p_role, p_status): (String, String, String, String, String) =
        sqlx::query_as(
            "SELECT m.role, m.status, p.id, p.role, p.status
             FROM club_memberships m
             JOIN pilots p ON p.id = m.pilot_id
             WHERE m.id = $1",
        )
        .bind(&accept.membership_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(m_role, "pilot");
    assert_eq!(m_status, "active");
    assert_eq!(p_role, "pilot");
    assert_eq!(p_status, "approved");

    let accept_audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM audit_log
         WHERE club_id = $1 AND actor_pilot_id = $2 AND action = 'accept_invite'
           AND entity_type = 'membership' AND entity_id = $3 AND detail = 'pilot'",
    )
    .bind(&club)
    .bind(&pid)
    .bind(&accept.membership_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(accept_audit_count, 1);
}

#[serial]
#[tokio::test]
async fn pilot_invite_approves_pending_pilot_and_preserves_existing_admin() {
    let db = open_db().await;
    let club = db.default_club_id().await.unwrap();
    ensure_live_club(&db, &club).await;
    let (inviter, _) = insert_admin(&db, &club, "inv-pres@test", "Inv").await;
    let now = now_ms();

    // 1) Pending pilot linked to account -> approved as pilot (not admin).
    let pending_email = "pending-pilot-inv@test";
    let pending_acct = db
        .ensure_account_for_email(pending_email, None, now)
        .await
        .unwrap();
    sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&pending_acct)
        .execute(&db.pool)
        .await
        .unwrap();
    let _ = db
        .join_club_with_account(&pending_acct, &club, "Pend", "", now)
        .await
        .unwrap();
    let raw_pend = db
        .invite_membership(&club, pending_email, &inviter, "pilot", now)
        .await
        .unwrap();
    let acc_pend = db.accept_membership_invite(&raw_pend, now).await.unwrap();
    assert_eq!(acc_pend.role, "pilot");
    let (m_role, m_status, p_status): (String, String, String) = sqlx::query_as(
        "SELECT m.role, m.status, p.status FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id WHERE m.id = $1",
    )
    .bind(&acc_pend.membership_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(m_role, "pilot");
    assert_eq!(m_status, "active");
    assert_eq!(p_status, "approved");

    // 2) Existing approved admin invited as pilot -> stays admin.
    let existing_admin = "already-admin-inv@test";
    let raw_adm = db
        .invite_membership(&club, existing_admin, &inviter, "admin", now)
        .await
        .unwrap();
    let acc_adm = db.accept_membership_invite(&raw_adm, now).await.unwrap();
    assert_eq!(acc_adm.role, "admin");

    let raw_demote_try = db
        .invite_membership(&club, existing_admin, &inviter, "pilot", now)
        .await
        .unwrap();
    let acc_after = db
        .accept_membership_invite(&raw_demote_try, now)
        .await
        .unwrap();
    assert_eq!(acc_after.role, "admin");
    let (m_role2, p_role2): (String, String) = sqlx::query_as(
        "SELECT m.role, p.role FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id WHERE m.id = $1",
    )
    .bind(&acc_after.membership_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(m_role2, "admin");
    assert_eq!(p_role2, "admin");
}
