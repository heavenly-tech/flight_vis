//! Flight-club registry: pilot auth, checkouts, shared live map.
mod access_request;
pub mod android_assetlinks;
mod auth;
mod auth_routes;
pub mod checkin;
mod checkin_landing;
mod club_scope;
mod contract_routes;
mod csrf;
pub mod database;
mod db;
mod db_access_pg;
mod db_auth_pg;
mod db_devices_pg;
mod db_kiosk_pg;
mod db_locale;
mod db_map_pg;
mod db_shares_pg;
mod dem;
mod device_tokens_import;
mod geo;
mod kiosk_qr;
pub mod mail;
mod mail_suppress;
mod map;
mod map_filter;
mod qr;
mod routes;
mod share_routes;
pub(crate) mod sse_limit;
mod visibility;
pub mod webauthn;

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod load_test;

pub use contract_routes::router as contract_router;
pub use db::{now_ms, ClubDb};
pub use routes::{patch_club_flight_by_id, post_club_flight_start, router};
