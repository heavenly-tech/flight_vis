//! Types shared by admin access APIs; `ClubDb` methods live in `db.rs`.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovePilotCheckout {
    #[serde(rename = "type")]
    pub type_code: String,
    pub tail: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Serialize)]
pub struct AdminPendingCounts {
    pub pilots: usize,
    pub access_requests: usize,
}
