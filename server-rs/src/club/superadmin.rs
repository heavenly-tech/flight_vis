//! Platform superadmin (email allowlist via `FLIGHT_SUPERADMIN_EMAILS`).

use std::collections::HashSet;

pub fn superadmin_emails_from_env() -> HashSet<String> {
    std::env::var("FLIGHT_SUPERADMIN_EMAILS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty() && s.contains('@'))
        .collect()
}

pub fn is_superadmin_email(emails: &HashSet<String>, email: &str) -> bool {
    emails.contains(&email.trim().to_ascii_lowercase())
}
