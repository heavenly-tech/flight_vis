//! Flight-club registry: pilot auth, checkouts, shared live map.
mod access_request;
pub mod android_assetlinks;
mod auth;
pub mod checkin;
mod checkin_landing;
mod club_scope;
pub mod database;
mod db;
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
mod visibility;

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod load_test;

pub use db::{now_ms, ClubDb};
pub use routes::{patch_club_flight_by_id, post_club_flight_start, router};
