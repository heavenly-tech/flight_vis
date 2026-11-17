//! Flight-club registry: SQLite persistence, pilot auth, checkouts, shared live map.
mod auth;
mod db;
mod map;
mod routes;
mod visibility;

#[cfg(test)]
mod api_tests;

pub use db::{now_ms, ClubDb};
pub use routes::{patch_club_flight_by_id, post_club_flight_start, router};