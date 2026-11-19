const RESERVED: &[&str] = &[
    "admin",
    "api",
    "app",
    "assets",
    "auth",
    "c",
    "cesiumstatic",
    "club",
    "default",
    "favicon.ico",
    "health",
    "live",
    "login",
    "logout",
    "map",
    "public",
    "register",
    "samples",
    "s",
    "screen",
    "static",
    "well-known",
];

pub fn normalize_slug(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

pub fn validate_slug(slug: &str) -> Result<(), &'static str> {
    let slug = normalize_slug(slug);
    if slug.len() < 2 || slug.len() > 48 {
        return Err("invalid slug length");
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("invalid slug characters");
    }
    if slug.starts_with('-') || slug.ends_with('-') || slug.contains("--") {
        return Err("invalid slug format");
    }
    if RESERVED.contains(&slug.as_str()) {
        return Err("reserved slug");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved() {
        assert!(validate_slug("api").is_err());
        assert!(validate_slug("auth").is_err());
    }

    #[test]
    fn accepts_club_slug() {
        assert!(validate_slug("heavenly").is_ok());
    }
}
