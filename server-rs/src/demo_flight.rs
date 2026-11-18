//! Canonical public demo track slug (disk store SPA: `GET /{slug}` and `GET /api/flights/{slug}`).

/// Current public demo flight slug (`https://flight.heavenly.cl/demo`).
pub const DEMO_FLIGHT_SLUG: &str = "demo";

/// Former demo slug; permanent redirect to [`DEMO_FLIGHT_SLUG`].
pub const LEGACY_DEMO_SLUG: &str = "vuelo-5";

pub fn redirect_path(base: &str, query: Option<&str>) -> String {
    match query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{base}?{q}"),
        None => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_keeps_query() {
        assert_eq!(redirect_path("/demo", Some("locale=es")), "/demo?locale=es");
        assert_eq!(
            redirect_path("/api/flights/demo", None),
            "/api/flights/demo"
        );
    }
}
