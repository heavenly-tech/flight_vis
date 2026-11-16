//! Device ingest tokens: `token -> device display name`. Constant-time verification;
//! tokens are never logged.

use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io, path::Path};
use subtle::ConstantTimeEq;

pub const MIN_TOKEN_LEN: usize = 32;

#[derive(Clone)]
pub struct DeviceAuth {
    entries: Vec<DeviceEntry>,
}

#[derive(Clone)]
struct DeviceEntry {
    token_digest: [u8; 32],
    name: String,
}

#[derive(Debug)]
pub enum DeviceAuthError {
    Io(io::Error),
    Parse(String),
}

impl std::fmt::Display for DeviceAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceAuthError::Io(e) => write!(f, "{e}"),
            DeviceAuthError::Parse(msg) => f.write_str(msg),
        }
    }
}

impl DeviceAuth {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn from_env() -> Result<Self, DeviceAuthError> {
        if let Ok(path_var) = std::env::var("FLIGHT_VIS_DEVICE_TOKENS_FILE") {
            let path = Path::new(path_var.trim());
            let raw = fs::read_to_string(path).map_err(DeviceAuthError::Io)?;
            return Self::parse_lines(&raw);
        }
        if let Ok(raw) = std::env::var("FLIGHT_VIS_DEVICE_TOKENS") {
            return Self::parse_lines(&raw);
        }
        Ok(Self::empty())
    }

    fn parse_lines(raw: &str) -> Result<Self, DeviceAuthError> {
        let mut token_map: HashMap<[u8; 32], String> = HashMap::new();
        let mut names = HashMap::new();
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
            if token.is_empty() {
                continue;
            }
            if name.is_empty() {
                eprintln!("warn: device token entry missing device name");
                continue;
            }
            if token.len() < MIN_TOKEN_LEN {
                return Err(DeviceAuthError::Parse(format!(
                    "device token must be at least {MIN_TOKEN_LEN} bytes"
                )));
            }
            if names.insert(name.to_string(), true).is_some() {
                return Err(DeviceAuthError::Parse(format!(
                    "duplicate device name: {name}"
                )));
            }
            let digest = digest_token(token);
            if token_map.insert(digest, name.to_string()).is_some() {
                return Err(DeviceAuthError::Parse("duplicate device token".to_string()));
            }
        }
        let entries = token_map
            .into_iter()
            .map(|(token_digest, name)| DeviceEntry { token_digest, name })
            .collect();
        Ok(Self { entries })
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
        Self::parse_lines(&lines).expect("test tokens")
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

    #[test]
    fn rejects_short_and_duplicate_names() {
        let short = DeviceAuth::parse_lines("short=name\n");
        assert!(matches!(short, Err(DeviceAuthError::Parse(_))));
        let dup = DeviceAuth::parse_lines(
            "0123456789abcdef0123456789abcdef=a\n0123456789abcdef0123456789abcde0=a\n",
        );
        assert!(matches!(dup, Err(DeviceAuthError::Parse(_))));
    }
}
