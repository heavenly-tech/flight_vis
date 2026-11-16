#[cfg(test)]
mod tests {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store,
        AppState, LoginThrottle,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const PASS: &str = "live-test-secret";

    async fn test_router() -> (axum::Router, std::path::PathBuf) {
        let data_dir = std::env::temp_dir().join(format!(
            "flight-vis-live-api-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&data_dir).await.unwrap();
        let dist_dir = data_dir.join("dist");
        tokio::fs::create_dir_all(&dist_dir).await.unwrap();
        let live = Arc::new(LiveEngine::new(&data_dir));
        live.load_from_disk().await.unwrap();
        let devices = DeviceAuth::with_tokens(&[(TOKEN, "pilot-phone")]);
        let (video_queue, queue_rx) =
            tokio::sync::mpsc::channel(crate::video::VIDEO_QUEUE_CAPACITY);
        crate::video::spawn_transcode_worker(Store::new(&data_dir), queue_rx);
        let state = AppState {
            dist_dir,
            auth: Auth::with_password(PASS),
            device_auth: devices,
            live,
            store: Store::new(&data_dir),
            video_queue,
            login_throttle: LoginThrottle::default(),
        };
        (build_router(state), data_dir)
    }

    #[tokio::test]
    async fn ingest_json_batch_and_auth_rejects() {
        let (router, data_dir) = test_router().await;
        let bad = Request::builder()
            .method("POST")
            .uri("/api/ingest")
            .header(header::AUTHORIZATION, "Bearer bad-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6}]}"#,
            ))
            .unwrap();
        let response = router.clone().oneshot(bad).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let good = Request::builder()
      .method("POST")
      .uri("/api/ingest")
      .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
      .header(header::CONTENT_TYPE, "application/json")
      .body(Body::from(
        r#"{"fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6,"alt":1200,"speed":10,"course":45,"accuracy":5}]}"#,
      ))
      .unwrap();
        let response = router.clone().oneshot(good).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn osmand_post_fixture_matches_traccar_client() {
        let (router, data_dir) = test_router().await;
        // Captured shape from Traccar Client Android ProtocolFormatter + RequestManager (POST, query on URL).
        let uri = format!(
      "/api/osmand?id={TOKEN}&timestamp=1609459200&lat=48.8566&lon=2.3522&speed=15&bearing=270&altitude=35&accuracy=10"
    );
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    #[test]
    fn sse_backfill_from_last_event_id() {
        use crate::live::sse::backfill_after;
        use crate::live::types::LiveFix;
        let fixes = vec![
            LiveFix {
                t: 1000,
                lat: 1.0,
                lon: 2.0,
                alt: None,
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: None,
            },
            LiveFix {
                t: 2000,
                lat: 1.0,
                lon: 2.0,
                alt: None,
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: None,
            },
            LiveFix {
                t: 3000,
                lat: 1.0,
                lon: 2.0,
                alt: None,
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: None,
            },
        ];
        let ids = backfill_after(&fixes, Some(1500));
        assert_eq!(ids, vec![2000, 3000]);
    }

    #[tokio::test]
    async fn sse_stream_requires_auth() {
        let (router, data_dir) = test_router().await;
        let ingest = Request::builder()
            .method("POST")
            .uri("/api/ingest")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"fixes":[{"t":1000,"lat":-33.40,"lon":-70.6}]}"#,
            ))
            .unwrap();
        router.clone().oneshot(ingest).await.unwrap();
        let list_req = Request::builder()
            .method("GET")
            .uri("/api/live")
            .body(Body::empty())
            .unwrap();
        let list_resp = router.clone().oneshot(list_req).await.unwrap();
        assert_eq!(list_resp.status(), StatusCode::UNAUTHORIZED);
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }
}
