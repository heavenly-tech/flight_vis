use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::http::HeaderMap;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const COOKIE: &str = "fv_club_session";
pub const SESSION_TTL_SEC: i64 = 7 * 24 * 3600;

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

pub fn new_id() -> String {
    new_session_id()
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
    cookie.push_str("; Max-Age=604800");
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
