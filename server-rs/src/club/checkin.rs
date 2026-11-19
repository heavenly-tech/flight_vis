use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

pub const CHECKIN_STEP_SEC: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckinTokenError {
    Format,
    Forged,
    OldStep,
}

pub fn current_step(now_ms: i64) -> i64 {
    now_ms / 1000 / CHECKIN_STEP_SEC
}

pub fn mint_checkin_token(secret: &str, club_id: &str, step: i64) -> String {
    let sig = sign_step(secret, club_id, step);
    format!("v1.{club_id}.{step}.{sig}")
}

fn sign_step(secret: &str, club_id: &str, step: i64) -> String {
    let msg = format!("v1:{club_id}:{step}");
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(msg.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn token_digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Club id embedded in a `v1.{club}.{step}.{sig}` token (no HMAC verify).
pub fn parse_checkin_token_club_id(token: &str) -> Option<String> {
    let token = token.trim();
    let mut parts = token.splitn(4, '.');
    let ver = parts.next()?;
    let club_id = parts.next()?;
    let step_s = parts.next()?;
    let _sig = parts.next()?;
    if ver != "v1" || club_id.is_empty() || parts.next().is_some() {
        return None;
    }
    step_s.parse::<i64>().ok()?;
    Some(club_id.to_string())
}

pub fn verify_checkin_token(
    secret: &str,
    token: &str,
    now_ms: i64,
) -> Result<(String, i64), CheckinTokenError> {
    let token = token.trim();
    let mut parts = token.splitn(4, '.');
    let (Some(ver), Some(club_id), Some(step_s), Some(sig)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(CheckinTokenError::Format);
    };
    if ver != "v1" || club_id.is_empty() || parts.next().is_some() {
        return Err(CheckinTokenError::Format);
    }
    let step: i64 = step_s.parse().map_err(|_| CheckinTokenError::Format)?;
    let cur = current_step(now_ms);
    if (step - cur).abs() > 1 {
        return Err(CheckinTokenError::OldStep);
    }
    let expected = sign_step(secret, club_id, step);
    let ok: bool = expected.as_bytes().ct_eq(sig.as_bytes()).into();
    if !ok {
        return Err(CheckinTokenError::Forged);
    }
    Ok((club_id.to_string(), step))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_window() {
        let secret = "club-secret";
        let club = "club-1";
        let now = 1_700_000_000_000_i64;
        let step = current_step(now);
        let tok = mint_checkin_token(secret, club, step);
        assert!(verify_checkin_token(secret, &tok, now).is_ok());
        let old = mint_checkin_token(secret, club, step - 2);
        assert_eq!(
            verify_checkin_token(secret, &old, now),
            Err(CheckinTokenError::OldStep)
        );
        let bad = mint_checkin_token("other", club, step);
        assert_eq!(
            verify_checkin_token(secret, &bad, now),
            Err(CheckinTokenError::Forged)
        );
    }
}
