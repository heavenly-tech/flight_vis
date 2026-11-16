use crate::{live::engine::SseSessionEvent, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use std::{convert::Infallible, pin::Pin, time::Duration};
use tokio::time;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream, IntervalStream};

const HEARTBEAT_SECS: u64 = 15;

pub(crate) fn backfill_after(
    fixes: &[crate::live::types::LiveFix],
    last_event_id: Option<i64>,
) -> Vec<i64> {
    fixes
        .iter()
        .filter(|f| last_event_id.is_none_or(|last| f.t > last))
        .map(|f| f.t)
        .collect()
}

pub async fn live_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(denied) = crate::require_auth(&state, &headers) {
        return denied.into_response();
    }

    let fixes = match state.live.get_fixes(&id).await {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({ "error": "not found" })),
            )
                .into_response();
        }
    };

    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());

    let backfill_ts = backfill_after(&fixes, last_event_id);
    let backfill: Vec<Event> = fixes
        .iter()
        .filter(|f| backfill_ts.contains(&f.t))
        .map(|f| {
            Event::default()
                .event("fix")
                .id(f.t.to_string())
                .json_data(f)
                .expect("fix json")
        })
        .collect();

    let rx = match state.live.subscribe(&id).await {
        Some(rx) => rx,
        None => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(json!({ "error": "not found" })),
            )
                .into_response();
        }
    };

    let live = BroadcastStream::new(rx).filter_map(|msg| {
        let event = match msg {
            Ok(SseSessionEvent::Fix(fix)) => Event::default()
                .event("fix")
                .id(fix.id)
                .json_data(fix.fix)
                .ok(),
            Ok(SseSessionEvent::End { id }) => Some(
                Event::default()
                    .event("end")
                    .data(json!({ "id": id }).to_string()),
            ),
            Err(BroadcastStreamRecvError::Lagged(_)) => None,
        };
        async move { event.map(Ok) }
    });

    let heartbeat = IntervalStream::new(time::interval(Duration::from_secs(HEARTBEAT_SECS)))
        .map(|_| Ok(Event::default().comment("heartbeat")));

    let merged: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(stream::select(
            stream::iter(backfill.into_iter().map(Ok)),
            stream::select(live, heartbeat),
        ));

    Sse::new(merged)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(HEARTBEAT_SECS)))
        .into_response()
}
