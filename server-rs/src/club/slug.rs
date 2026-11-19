/// Slugs that collide with Axum routes (`main.rs`, club routers) or dist top-level paths.
pub const RESERVED: &[&str] = &[
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
    "icons",
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

/// Top-level first path segments served by the Axum app (keep aligned with `RESERVED`).
#[allow(dead_code)]
pub const ROUTER_TOP_LEVEL_SEGMENTS: &[&str] = &[
    "admin",
    "api",
    "app",
    "assets",
    "auth",
    "c",
    "cesiumstatic",
    "club",
    "default",
    "health",
    "icons",
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

/// Map confusable glyphs/sequences so lookalike slugs collide (cas ≈ ca5, rn ≈ m, vv ≈ w).
pub fn confusable_slug_skeleton(slug: &str) -> String {
    let s = normalize_slug(slug);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'r' && bytes[i + 1] == b'n' {
            out.push(b'm');
            i += 2;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'v' && bytes[i + 1] == b'v' {
            out.push(b'w');
            i += 2;
            continue;
        }
        let mapped = match bytes[i] {
            b'0' => b'o',
            b'1' => b'l',
            b'5' => b's',
            c => c,
        };
        out.push(mapped);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
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

/// Whether a normalized club slug collides with an existing flight data directory slug.
pub fn slug_conflicts_with_flight(slug: &str, flight_slugs: &[String]) -> bool {
    let slug = normalize_slug(slug);
    if slug.is_empty() {
        return false;
    }
    flight_slugs.iter().any(|f| normalize_slug(f) == slug)
}

/// Static slug rules plus flight slug reservation (signup, rename, seed).
pub fn validate_club_slug_candidate(
    slug: &str,
    flight_slugs: &[String],
) -> Result<(), &'static str> {
    validate_slug(slug)?;
    if slug_conflicts_with_flight(slug, flight_slugs) {
        return Err("flight slug collision");
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
    fn router_top_level_segments_reserved_for_club_slugs() {
        for seg in ROUTER_TOP_LEVEL_SEGMENTS {
            assert!(
                validate_slug(seg).is_err(),
                "router segment `{seg}` must be reserved for club slugs"
            );
        }
    }

    #[test]
    fn confusable_skeleton_collapses_lookalikes() {
        assert_eq!(
            confusable_slug_skeleton("cas"),
            confusable_slug_skeleton("ca5")
        );
        assert_eq!(
            confusable_slug_skeleton("evil"),
            confusable_slug_skeleton("evi1")
        );
        assert_eq!(
            confusable_slug_skeleton("foo"),
            confusable_slug_skeleton("fo0")
        );
        assert_eq!(
            confusable_slug_skeleton("avvy"),
            confusable_slug_skeleton("awy")
        );
        assert_eq!(
            confusable_slug_skeleton("acme"),
            confusable_slug_skeleton("acrne")
        );
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
            "icons",
        ] {
            assert!(validate_slug(slug).is_err(), "expected reserved: {slug}");
        }
        for slug in ["c", "s"] {
            assert!(validate_slug(slug).is_err(), "expected blocked: {slug}");
        }
    }

    #[test]
    fn accepts_club_slug() {
        assert!(validate_slug("heavenly").is_ok());
    }
}
