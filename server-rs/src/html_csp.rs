//! Per-page HTML Content-Security-Policy (strict vs globe). Keep in sync with
//! `scripts/html-csp-routes.mjs` and Playwright `e2e/csp-constants.ts`.

/// Strict policy (auth, admin, club forms). Matches `server-rs/csp/html-strict.csp`.
pub const HTML_STRICT_CSP: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/csp/html-strict.csp"));

/// Globe / map viewer policy. Matches `server-rs/csp/html-globe.csp`.
pub const HTML_GLOBE_CSP: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/csp/html-globe.csp"));

/// Token- or secret-bearing HTML (magic links, share URLs, QR). Do not leak path/query via Referer.
pub const SENSITIVE_REFERRER_POLICY: &str = "no-referrer";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlCspKind {
    Strict,
    Globe,
}

/// Every HTML response path that sets CSP. Paths are normalized (`/login`, `/foo.html`).
///
/// Dynamic handlers (e.g. `/s/{token}`, `/c/{slug}/registro`) use the same policy as their
/// dist template row (`share-flight.html`, `club-register.html`, …).
///
/// Unknown `dist/*.html` files without a table row return HTTP 500 from static file serving (fail closed).
pub const HTML_CSP_ROUTE_TABLE: &[(&str, HtmlCspKind)] = &[
    // Globe: Cesium viewer (main SPA, shared flight view)
    ("/index.html", HtmlCspKind::Globe),
    ("/__spa_index__", HtmlCspKind::Globe),
    ("/club-globe-screen.html", HtmlCspKind::Globe),
    ("/share-flight.html", HtmlCspKind::Globe),
    // Strict: auth, account, admin, lists, club registration, QR / check-in, e2e fixtures
    ("/login", HtmlCspKind::Strict),
    ("/login.html", HtmlCspKind::Strict),
    ("/login-legacy.html", HtmlCspKind::Strict),
    ("/signup", HtmlCspKind::Strict),
    ("/account-clubs.html", HtmlCspKind::Strict),
    ("/account.html", HtmlCspKind::Strict),
    ("/api/account/auth/magic", HtmlCspKind::Strict),
    ("/api/club/auth/magic", HtmlCspKind::Strict),
    ("/app.html", HtmlCspKind::Strict),
    ("/club-pilot.html", HtmlCspKind::Strict),
    ("/club-flight.html", HtmlCspKind::Strict),
    ("/club-register.html", HtmlCspKind::Strict),
    ("/club-signup.html", HtmlCspKind::Strict),
    ("/club-suspended.html", HtmlCspKind::Strict),
    ("/club-admin-types.html", HtmlCspKind::Strict),
    ("/club-admin-tails.html", HtmlCspKind::Strict),
    ("/club-admin-pilots.html", HtmlCspKind::Strict),
    ("/club-admin-checkouts.html", HtmlCspKind::Strict),
    ("/club-admin-map.html", HtmlCspKind::Strict),
    ("/club-admin-kiosk-layout.html", HtmlCspKind::Strict),
    ("/club-admin-requests.html", HtmlCspKind::Strict),
    ("/club-admin-checkin-qr.html", HtmlCspKind::Strict),
    ("/club-map.html", HtmlCspKind::Strict),
    ("/public-map.html", HtmlCspKind::Strict),
    ("/club-screen.html", HtmlCspKind::Strict),
    ("/e2e-live.html", HtmlCspKind::Strict),
    ("/club-checkin-landing.html", HtmlCspKind::Strict),
    ("/club-checkin-expired.html", HtmlCspKind::Strict),
];

/// Built `dist` HTML entrypoint filenames (must each have a `/name.html` row above).
#[allow(dead_code)] // covered by `every_dist_html_file_has_route_entry` unit test
pub const DIST_HTML_FILES: &[&str] = &[
    "index.html",
    "login.html",
    "login-legacy.html",
    "account-clubs.html",
    "account.html",
    "app.html",
    "club-globe-screen.html",
    "club-pilot.html",
    "club-flight.html",
    "club-map.html",
    "public-map.html",
    "club-register.html",
    "club-signup.html",
    "club-suspended.html",
    "club-screen.html",
    "club-admin-types.html",
    "club-admin-tails.html",
    "club-admin-pilots.html",
    "club-admin-checkouts.html",
    "club-admin-checkin-qr.html",
    "club-admin-map.html",
    "club-admin-kiosk-layout.html",
    "club-admin-requests.html",
    "club-checkin-landing.html",
    "club-checkin-expired.html",
    "share-flight.html",
    "e2e-live.html",
];

pub fn csp_string(kind: HtmlCspKind) -> &'static str {
    match kind {
        HtmlCspKind::Strict => HTML_STRICT_CSP,
        HtmlCspKind::Globe => HTML_GLOBE_CSP,
    }
}

fn normalize_path(path: &str) -> String {
    let path_only = path.split('?').next().unwrap_or(path).trim();
    if path_only.is_empty() {
        return "/".to_string();
    }
    if path_only.starts_with('/') {
        path_only.to_string()
    } else {
        format!("/{path_only}")
    }
}

pub fn policy_for_route(path: &str) -> Option<HtmlCspKind> {
    let norm = normalize_path(path);
    HTML_CSP_ROUTE_TABLE
        .iter()
        .find(|(route, _)| *route == norm.as_str())
        .map(|(_, k)| *k)
}

/// CSP for a file under `dist/` (e.g. `login.html` or `assets/../login.html` guard — basename only).
pub fn csp_for_dist_relative(relative: &str) -> Result<&'static str, ()> {
    let base = relative
        .split('/')
        .next_back()
        .filter(|s| !s.is_empty())
        .ok_or(())?;
    if !base.ends_with(".html") {
        return Err(());
    }
    let route = format!("/{base}");
    let kind = policy_for_route(&route).ok_or(())?;
    Ok(csp_string(kind))
}

pub fn csp_for_spa_fallback() -> &'static str {
    csp_string(policy_for_route("/__spa_index__").expect("__spa_index__ in HTML_CSP_ROUTE_TABLE"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN_D4281D3_STRICT: &str = "default-src 'self'; img-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

    #[test]
    fn html_strict_matches_main_d4281d3() {
        assert_eq!(HTML_STRICT_CSP.trim(), MAIN_D4281D3_STRICT);
    }

    #[test]
    fn globe_policy_has_wasm_and_arcgis() {
        assert!(HTML_GLOBE_CSP.contains("'wasm-unsafe-eval'"));
        assert!(HTML_GLOBE_CSP.contains("https://services.arcgisonline.com"));
        assert!(HTML_GLOBE_CSP.contains("https://elevation3d.arcgis.com"));
    }

    #[test]
    fn strict_policy_has_no_wasm_or_third_party_hosts() {
        assert!(!HTML_STRICT_CSP.contains("wasm-unsafe-eval"));
        assert!(!HTML_STRICT_CSP.contains("https://"));
        assert!(!HTML_STRICT_CSP.contains("blob:"));
    }

    #[test]
    fn every_dist_html_file_has_route_entry() {
        for file in DIST_HTML_FILES {
            let route = format!("/{file}");
            assert!(
                policy_for_route(&route).is_some(),
                "missing route table entry for {route}"
            );
        }
    }

    #[test]
    fn route_table_has_no_duplicate_paths() {
        let mut seen = std::collections::HashSet::new();
        for (path, _) in HTML_CSP_ROUTE_TABLE {
            assert!(seen.insert(*path), "duplicate CSP route {path}");
        }
    }

    #[test]
    fn login_and_account_pages_are_strict() {
        for path in [
            "/login",
            "/signup",
            "/account-clubs.html",
            "/club-admin-types.html",
        ] {
            assert_eq!(policy_for_route(path), Some(HtmlCspKind::Strict));
        }
    }

    #[test]
    fn globe_pages_use_globe_policy() {
        for path in ["/index.html", "/share-flight.html", "/__spa_index__"] {
            assert_eq!(policy_for_route(path), Some(HtmlCspKind::Globe));
        }
    }

    #[test]
    fn list_map_pages_use_strict_policy() {
        for path in [
            "/club-map.html",
            "/public-map.html",
            "/club-screen.html",
            "/e2e-live.html",
        ] {
            assert_eq!(policy_for_route(path), Some(HtmlCspKind::Strict));
        }
    }

    #[test]
    fn vite_lists_every_dist_html_file() {
        let vite = std::fs::read_to_string("../vite.config.ts").expect("vite.config.ts");
        for file in DIST_HTML_FILES {
            assert!(vite.contains(file), "{file} missing from vite build inputs");
        }
    }

    #[test]
    fn every_web_source_html_file_has_route_entry() {
        let web_source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut html_files: Vec<String> = std::fs::read_dir(&web_source_dir)
            .expect("read web source dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".html"))
            .collect();
        html_files.sort();
        assert!(
            !html_files.is_empty(),
            "no HTML files found in web source dir"
        );
        for name in &html_files {
            let route = format!("/{name}");
            assert!(
                policy_for_route(&route).is_some(),
                "missing CSP route table entry for {route}"
            );
            assert!(
                DIST_HTML_FILES.contains(&name.as_str()),
                "{name} missing from DIST_HTML_FILES"
            );
        }
    }
}
