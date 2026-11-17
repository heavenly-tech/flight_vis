//! Flight-club registry: pilot auth, checkouts, shared live map.
mod access_request;
pub mod android_assetlinks;
mod auth;
pub mod checkin;
mod checkin_landing;
mod club_scope;
pub mod database;
mod db;
mod db_access;
mod db_checkin;
mod db_devices;
mod db_kiosk_layout;
mod db_map_view;
mod db_shares;
mod dem;
mod geo;
mod kiosk_qr;
pub mod mail;
mod mail_suppress;
mod map;
mod map_filter;
mod qr;
mod routes;
mod share_routes;
pub(crate) mod sqlite_import;
mod visibility;

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod load_test;

pub use db::{now_ms, ClubDb};
pub use routes::{patch_club_flight_by_id, post_club_flight_start, router};
