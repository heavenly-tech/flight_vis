use crate::{
    live::{engine::SseSessionEvent, types::SessionPhase},
    AppState,
};
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
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};

const HEARTBEAT_SECS: u64 = 15;

#[cfg(test)]
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

fn backfill_events(
    fixes: &[crate::live::types::LiveFix],
    last_event_id: Option<i64>,
) -> Vec<Event> {
    fixes
        .iter()
        .filter(|f| last_event_id.is_none_or(|last| f.t > last))
        .map(|f| {
            Event::default()
                .event("fix")
                .id(f.t.to_string())
                .json_data(f)
                .expect("fix json")
        })
        .collect()
}

fn end_event(session_id: &str) -> Event {
    Event::default()
        .event("end")
        .data(json!({ "id": session_id }).to_string())
}

pub async fn live_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(denied) = crate::require_auth(&state, &headers).await {
        return denied.into_response();
    }

    let snapshot = match state.live.stream_snapshot(&id).await {
        Some(snapshot) => snapshot,
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

    let backfill = backfill_events(&snapshot.fixes, last_event_id);

    if snapshot.phase == SessionPhase::Ended {
        let merged: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(
            stream::iter(backfill.into_iter().map(Ok)).chain(stream::iter([Ok(end_event(&id))])),
        );
        return Sse::new(merged).into_response();
    }

    let rx = match snapshot.live_rx {
        Some(rx) => rx,
        None => {
            let merged: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(
                stream::iter(backfill.into_iter().map(Ok))
                    .chain(stream::iter([Ok(end_event(&id))])),
            );
            return Sse::new(merged).into_response();
        }
    };

    let stream_id = id.clone();
    let live = stream::unfold(Some(BroadcastStream::new(rx)), move |rx_opt| {
        let stream_id = stream_id.clone();
        async move {
            let mut rx = rx_opt?;
            let msg = rx.next().await?;
            let (event, next_rx) = match msg {
                Ok(SseSessionEvent::Fix(fix)) => (
                    Event::default()
                        .event("fix")
                        .id(fix.id)
                        .json_data(fix.fix)
                        .ok()?,
                    Some(rx),
                ),
                Ok(SseSessionEvent::End) => (end_event(&stream_id), None),
                Err(BroadcastStreamRecvError::Lagged(_)) => return None,
            };
            Some((Ok(event), next_rx))
        }
    });

    let merged: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(stream::iter(backfill.into_iter().map(Ok)).chain(live));

    Sse::new(merged)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(HEARTBEAT_SECS)))
        .into_response()
}
