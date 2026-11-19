//! Flight-club registry: pilot auth, checkouts, shared live map.
mod access_request;
mod account_routes;
mod admin_membership_routes;
pub mod android_assetlinks;
mod auth;
pub use auth::parse_fv_locale_cookie;
mod auth_routes;
pub mod checkin;
mod checkin_landing;
mod club_scope;
mod contract_routes;
pub mod csrf;
pub mod database;
mod db;
mod db_access_pg;
mod db_account;
mod db_auth_pg;
pub(crate) mod db_club_lifecycle;
mod db_devices_pg;
mod db_kiosk_pg;
mod db_locale;
mod db_map_pg;
mod db_memberships_admin;
mod db_shares_pg;
mod dem;
mod device_tokens_import;
mod geo;
mod kiosk_qr;
pub mod mail;
mod mail_suppress;
mod map;
mod map_filter;
mod membership_status;
mod qr;
mod routes;
mod share_routes;
mod signup_routes;
pub(crate) mod slug;
pub(crate) mod sse_limit;
pub(crate) mod superadmin;
mod superadmin_routes;
mod visibility;
pub mod webauthn;

#[cfg(test)]
mod account_integration_tests;
#[cfg(test)]
mod account_security_followup_tests;
#[cfg(test)]
mod account_takeover_tests;
#[cfg(test)]
mod admin_membership_http_tests;
#[cfg(test)]
mod admin_membership_integration_tests;
#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod club_security_tests;
#[cfg(test)]
mod club_signup_tests;
#[cfg(test)]
mod cross_site_csrf_tests;
#[cfg(test)]
mod load_test;

pub use auth::parse_session_cookie;
pub use contract_routes::router as contract_router;
pub use db::{now_ms, ClubDb};
pub use routes::{patch_club_flight_by_id, post_club_flight_start, router};
