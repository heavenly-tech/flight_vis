#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct ClubContext {
    pub club_id: String,
    pub slug: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubScopeError {
    NotFound,
    Forbidden,
}

pub fn checkin_qr_path(slug: &str, token: &str) -> String {
    format!("/c/{slug}/q/{token}")
}