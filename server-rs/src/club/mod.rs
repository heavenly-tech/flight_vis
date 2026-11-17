//! Flight-club registry: pilot auth, checkouts, shared live map.
mod access_request;
mod auth;
pub mod checkin;
pub mod database;
mod db;
mod db_access;
mod map;
mod qr;
mod routes;
pub(crate) mod sqlite_import;
mod visibility;

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod load_test;

pub use db::{now_ms, ClubDb};
pub use routes::{patch_club_flight_by_id, post_club_flight_start, router};
