mod engine;
mod ingest;
mod persist;
mod routes;
mod sse;
mod types;

pub use engine::LiveEngine;
#[cfg(test)]
pub use engine::{
    SseSessionEvent, CLUB_INGEST_WINDOW_MS, MAX_CLUB_INGEST_FIXES_PER_MINUTE,
    MAX_INGEST_FLIGHT_MISMATCH_PER_MINUTE,
};
pub(crate) use ingest::effective_pre_start_min_t;
#[cfg(test)]
pub use ingest::{MAX_INGEST_BATCH_FIXES, PRE_START_SKEW_MS};
pub use routes::router;
pub use types::LiveFix;

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod engine_tests;
