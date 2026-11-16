//! Device ingest tokens: `token -> device display name`. Constant-time verification;
//! tokens are never logged.

use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, path::Path};
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct DeviceAuth {
    entries: Vec<DeviceEntry>,
}

#[derive(Clone)]
struct DeviceEntry {
    token_digest: [u8; 32],
    name: String,
}

impl DeviceAuth {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn from_env() -> Self {
        if let Ok(path) = std::env::var("FLIGHT_VIS_DEVICE_TOKENS_FILE") {
            let path = Path::new(path.trim());
            if let Ok(raw) = fs::read_to_string(path) {
                return Self::parse_lines(&raw);
            }
        }
        if let Ok(raw) = std::env::var("FLIGHT_VIS_DEVICE_TOKENS") {
            return Self::parse_lines(&raw);
        }
        Self::empty()
    }

    fn parse_lines(raw: &str) -> Self {
        let mut map: HashMap<[u8; 32], String> = HashMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (token, name) = match line.split_once('=') {
                Some(pair) => pair,
                None => match line.split_once(|c: char| c.is_whitespace()) {
                    Some(pair) => pair,
                    None => continue,
                },
            };
            let token = token.trim();
            let name = name.trim();
            if token.is_empty() || name.is_empty() {
                continue;
            }
            map.insert(digest_token(token), name.to_string());
        }
        let entries = map
            .into_iter()
            .map(|(token_digest, name)| DeviceEntry { token_digest, name })
            .collect();
        Self { entries }
    }

    pub fn verify(&self, token: &str) -> Option<&str> {
        if token.is_empty() {
            return None;
        }
        let candidate = digest_token(token);
        for entry in &self.entries {
            if bool::from(entry.token_digest.ct_eq(&candidate)) {
                return Some(entry.name.as_str());
            }
        }
        None
    }

    #[cfg(test)]
    pub fn with_tokens(pairs: &[(&str, &str)]) -> Self {
        let mut lines = String::new();
        for (token, name) in pairs {
            lines.push_str(token);
            lines.push('=');
            lines.push_str(name);
            lines.push('\n');
        }
        Self::parse_lines(&lines)
    }
}

fn digest_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_and_empty() {
        let auth = DeviceAuth::with_tokens(&[("0123456789abcdef0123456789abcdef", "pilot-phone")]);
        assert!(auth.verify("").is_none());
        assert!(auth.verify("wrong").is_none());
        assert_eq!(
            auth.verify("0123456789abcdef0123456789abcdef"),
            Some("pilot-phone")
        );
    }
}
