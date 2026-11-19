/// Slugs that collide with Axum routes (`main.rs`, club routers) or dist top-level paths.
const RESERVED: &[&str] = &[
    // main.rs + shared routers
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
    "s",
    "screen",
    "signup",
    "superadmin",
    "well-known",
    "www",
    // static fallback (`dist/` top-level)
    "demo",
    "geoid",
    "index",
    "locales",
    "samples",
    "static",
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
        for slug in RESERVED {
            assert!(validate_slug(slug).is_err(), "expected reserved: {slug}");
        }
    }

    #[test]
    fn rejects_platform_top_level_paths() {
        for slug in [
            "api",
            "admin",
            "login",
            "map",
            "app",
            "static",
            "assets",
            "signup",
            "club",
            "logout",
            "live",
            "register",
            "public",
            "auth",
            "health",
            "samples",
            "geoid",
            "locales",
            "index",
            "cesiumstatic",
            "superadmin",
            "demo",
            "www",
        ] {
            assert!(validate_slug(slug).is_err(), "expected reserved: {slug}");
        }
        // single-segment path prefixes (also in RESERVED; invalid length if < 2 chars)
        for slug in ["c", "s"] {
            assert!(validate_slug(slug).is_err(), "expected blocked: {slug}");
        }
    }

    #[test]
    fn accepts_club_slug() {
        assert!(validate_slug("heavenly").is_ok());
    }
}
