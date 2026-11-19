use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::http::HeaderMap;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const COOKIE: &str = "fv_club_session";
pub const ACCOUNT_COOKIE: &str = "fv_account_session";
/// `__Host-` prefix: Path=/, Secure, no Domain (set/clear helpers enforce this).
pub const SIGNUP_NONCE_COOKIE: &str = "__Host-fv_account_signup_nonce";
pub const ACCOUNT_PASSWORD_MAX_CHARS: usize = 128;
/// bcrypt only uses the first 72 bytes; reject longer UTF-8 passwords for parity.
pub const ACCOUNT_PASSWORD_MAX_BYTES: usize = 72;
pub const DISPLAY_COOKIE: &str = "fv_club_display";
pub const LOCALE_COOKIE: &str = "fv_locale";
pub const SESSION_TTL_SEC: i64 = 12 * 3600;

/// Argon2 hash of a fixed password; verified on failed login lookups for timing parity.
#[allow(dead_code)]
pub const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHlzYWx0c2FsdDE$+7K0dWSyNpY2LxqJX5v+fxWuZLq0sP3Q8xZ7d3Y6o2w";

pub fn hash_password(password: &str) -> Result<String, &'static str> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| "hash failed")
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn digest_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn new_device_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn new_share_token() -> String {
    new_device_token()
}

pub fn new_id() -> String {
    new_session_id()
}

pub fn parse_fv_locale_cookie(headers: &HeaderMap) -> Option<String> {
    let header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())?;
    for part in header.split(';') {
        let part = part.trim();
        let mut kv = part.splitn(2, '=');
        let (Some(name), Some(value)) = (kv.next(), kv.next()) else {
            continue;
        };
        if name == LOCALE_COOKIE {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub fn parse_display_cookie(headers: &HeaderMap) -> Option<String> {
    let header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())?;
    for part in header.split(';') {
        let part = part.trim();
        let mut kv = part.splitn(2, '=');
        let (Some(name), Some(value)) = (kv.next(), kv.next()) else {
            continue;
        };
        if name == DISPLAY_COOKIE {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub fn set_display_cookie(token: &str, secure: bool) -> String {
    let mut cookie = format!("{DISPLAY_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie.push_str("; Max-Age=31536000");
    cookie
}

#[allow(dead_code)]
pub fn clear_display_cookie(secure: bool) -> String {
    let mut cookie = format!("{DISPLAY_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub fn parse_account_session_cookie(headers: &HeaderMap) -> Option<String> {
    let header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())?;
    for part in header.split(';') {
        let part = part.trim();
        let mut kv = part.splitn(2, '=');
        let (Some(name), Some(value)) = (kv.next(), kv.next()) else {
            continue;
        };
        if name == ACCOUNT_COOKIE {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub fn set_account_session_cookie(session_id: &str, secure: bool) -> String {
    let mut cookie = format!("{ACCOUNT_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie.push_str(&format!("; Max-Age={SESSION_TTL_SEC}"));
    cookie
}

#[allow(dead_code)]
pub fn clear_account_session_cookie(secure: bool) -> String {
    let mut cookie = format!("{ACCOUNT_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub fn parse_signup_nonce_cookie(headers: &HeaderMap) -> Option<String> {
    let header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())?;
    for part in header.split(';') {
        let part = part.trim();
        let mut kv = part.splitn(2, '=');
        let (Some(name), Some(value)) = (kv.next(), kv.next()) else {
            continue;
        };
        if name == SIGNUP_NONCE_COOKIE {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub fn set_signup_nonce_cookie(nonce: &str, _secure: bool) -> String {
    format!("{SIGNUP_NONCE_COOKIE}={nonce}; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=900")
}

pub fn clear_signup_nonce_cookie(_secure: bool) -> String {
    format!("{SIGNUP_NONCE_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=0")
}

pub fn parse_session_cookie(headers: &HeaderMap) -> Option<String> {
    let header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())?;
    for part in header.split(';') {
        let part = part.trim();
        let mut kv = part.splitn(2, '=');
        let (Some(name), Some(value)) = (kv.next(), kv.next()) else {
            continue;
        };
        if name == COOKIE {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub fn set_session_cookie(session_id: &str, secure: bool) -> String {
    let mut cookie = format!("{COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie.push_str(&format!("; Max-Age={SESSION_TTL_SEC}"));
    cookie
}

pub fn clear_session_cookie(secure: bool) -> String {
    let mut cookie = format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn token_digest_stable() {
        assert_eq!(digest_token("abc"), digest_token("abc"));
    }
}
