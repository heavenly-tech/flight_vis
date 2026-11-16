use crate::live::persist::{LivePersist, SessionRecord};
use crate::live::types::{
    valid_lat_lon, LiveFix, LiveSessionSummary, SessionPhase, ALT_REF_WGS84_ELLIPSOID,
};
use rand::RngCore;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{broadcast, RwLock};

pub const SESSION_IDLE_END_MS: i64 = 10 * 60 * 1000;
pub const SESSION_RETAIN_MS: i64 = 24 * 60 * 60 * 1000;
pub const MAX_FIXES_PER_SESSION: usize = 12 * 60 * 60; // 12 h at 1 Hz
pub const MAX_CONCURRENT_SESSIONS: usize = 32;
pub const MAX_FUTURE_MS: i64 = 60 * 1000;
pub const MIN_FIX_MS: i64 = 1_577_836_800_000; // 2020-01-01 UTC
pub const MAX_INGEST_BATCH: usize = 120;
pub const MAX_INGEST_PER_MINUTE: usize = 600;

#[derive(Clone, Debug)]
pub struct SseFixEvent {
    pub id: String,
    pub fix: LiveFix,
}

#[derive(Clone, Debug)]
pub enum SseSessionEvent {
    Fix(SseFixEvent),
    #[allow(dead_code)]
    End { id: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum AckResult {
    Ok,
    NotFound,
    StillActive,
}

struct SessionRuntime {
    record: SessionRecord,
    fixes: Vec<LiveFix>,
    seen_t: HashSet<i64>,
    bus: broadcast::Sender<SseSessionEvent>,
}

pub struct LiveEngine {
    persist: LivePersist,
    sessions: RwLock<HashMap<String, SessionRuntime>>,
    device_active: RwLock<HashMap<String, String>>,
    ingest_hits: RwLock<HashMap<String, Vec<i64>>>,
    clock_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl LiveEngine {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self::with_clock(data_dir, Arc::new(system_now_ms))
    }

    pub fn with_clock(
        data_dir: &std::path::Path,
        clock_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            persist: LivePersist::new(data_dir),
            sessions: RwLock::new(HashMap::new()),
            device_active: RwLock::new(HashMap::new()),
            ingest_hits: RwLock::new(HashMap::new()),
            clock_ms,
        }
    }

    pub fn now_ms(&self) -> i64 {
        (self.clock_ms)()
    }

    pub async fn load_from_disk(&self) -> std::io::Result<()> {
        self.persist.ensure_dir().await?;
        let mut index = match self.persist.load_index().await {
            Ok(index) => index,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => HashMap::new(),
            Err(error) => return Err(error),
        };
        let orphan_ids = self.persist.list_orphan_log_ids(&index).await?;
        if !orphan_ids.is_empty() {
            let hints = self.persist.device_hints_from_broken_index().await;
            let now = self.now_ms();
            for id in orphan_ids {
                let fixes = self.persist.read_fixes(&id).await?;
                if fixes.is_empty() {
                    continue;
                }
                let started_at = fixes.first().map(|f| f.t).unwrap_or(now);
                let last_fix_at = fixes.last().map(|f| f.t);
                let device = hints
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "recovered".to_string());
                index.insert(
                    id.clone(),
                    SessionRecord {
                        id,
                        device,
                        phase: SessionPhase::Ended,
                        started_at,
                        ended_at: Some(now),
                        last_fix_at,
                        last_rx_at: Some(now),
                        fix_count: fixes.len(),
                        acked: false,
                    },
                );
            }
            self.persist.replace_index(&index).await?;
        }
        let mut sessions = self.sessions.write().await;
        let mut device_active = self.device_active.write().await;
        sessions.clear();
        device_active.clear();
        for (id, mut record) in index.clone() {
            let fixes = self.persist.read_fixes(&id).await?;
            let mut seen_t = HashSet::with_capacity(fixes.len());
            for f in &fixes {
                seen_t.insert(f.t);
            }
            record.fix_count = fixes.len();
            if record.last_rx_at.is_none() {
                record.last_rx_at = record.last_fix_at.or(Some(record.started_at));
            }
            let (bus, _) = broadcast::channel(256);
            let runtime = SessionRuntime {
                record,
                fixes,
                seen_t,
                bus,
            };
            if runtime.record.phase == SessionPhase::Active {
                device_active.insert(runtime.record.device.clone(), id.clone());
            }
            sessions.insert(id, runtime);
        }
        Ok(())
    }

    fn session_id() -> String {
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(&bytes)
    }

    pub async fn allow_ingest(&self, device_key: &str) -> bool {
        let now = self.now_ms();
        let cutoff = now - 60_000;
        let mut hits = self.ingest_hits.write().await;
        let entry = hits.entry(device_key.to_string()).or_default();
        entry.retain(|t| *t > cutoff);
        if entry.len() >= MAX_INGEST_PER_MINUTE {
            return false;
        }
        entry.push(now);
        if hits.len() > 4096 {
            hits.retain(|_, v| v.last().is_some_and(|t| *t > cutoff));
        }
        true
    }

    pub fn validate_fix(&self, fix: &LiveFix) -> Result<(), &'static str> {
        if !valid_lat_lon(fix.lat, fix.lon) {
            return Err("invalid lat/lon");
        }
        if fix.t < MIN_FIX_MS {
            return Err("timestamp too old");
        }
        let now = self.now_ms();
        if fix.t > now + MAX_FUTURE_MS {
            return Err("timestamp too far in future");
        }
        Ok(())
    }

    pub async fn ingest_fixes(
        &self,
        device_name: &str,
        inputs: Vec<LiveFix>,
    ) -> Result<usize, &'static str> {
        if inputs.len() > MAX_INGEST_BATCH {
            return Err("batch too large");
        }
        for fix in &inputs {
            self.validate_fix(fix)?;
        }

        let mut accepted = 0usize;
        let mut batch = inputs;
        while !batch.is_empty() {
            let session_id = self.ensure_active_session(device_name).await?;
            let mut sessions = self.sessions.write().await;
            let Some(runtime) = sessions.get_mut(&session_id) else {
                return Err("session missing");
            };
            if runtime.record.phase != SessionPhase::Active {
                drop(sessions);
                continue;
            }

            let mut remaining = Vec::new();
            let rx_now = self.now_ms();
            for fix in batch {
                if runtime.record.fix_count >= MAX_FIXES_PER_SESSION {
                    remaining.push(fix);
                    continue;
                }
                if !runtime.seen_t.insert(fix.t) {
                    runtime.record.last_rx_at = Some(rx_now);
                    continue;
                }
                let pos = runtime.fixes.partition_point(|f| f.t < fix.t);
                runtime.fixes.insert(pos, fix.clone());
                runtime.record.fix_count += 1;
                runtime.record.last_rx_at = Some(rx_now);
                runtime.record.last_fix_at = Some(
                    runtime
                        .record
                        .last_fix_at
                        .map(|prev| prev.max(fix.t))
                        .unwrap_or(fix.t),
                );
                accepted += 1;
                let _ = self.persist.append_fix(&session_id, &fix).await;
                let _ = runtime.bus.send(SseSessionEvent::Fix(SseFixEvent {
                    id: fix.t.to_string(),
                    fix: fix.clone(),
                }));
            }

            let at_cap = runtime.record.fix_count >= MAX_FIXES_PER_SESSION;
            let record = runtime.record.clone();
            drop(sessions);

            let index = self.snapshot_index().await;
            let _ = self.persist.upsert_session(&index, &record).await;

            if at_cap && !remaining.is_empty() {
                let _ = self.end_session(&session_id).await;
                batch = remaining;
                continue;
            }
            break;
        }
        Ok(accepted)
    }

    async fn ensure_active_session(&self, device_name: &str) -> Result<String, &'static str> {
        let mut sessions = self.sessions.write().await;
        let mut device_active = self.device_active.write().await;

        if let Some(id) = device_active.get(device_name) {
            if let Some(runtime) = sessions.get(id) {
                if runtime.record.phase == SessionPhase::Active {
                    return Ok(id.clone());
                }
            }
            device_active.remove(device_name);
        }

        let active_count = sessions
            .values()
            .filter(|s| s.record.phase == SessionPhase::Active)
            .count();
        if active_count >= MAX_CONCURRENT_SESSIONS {
            return Err("too many concurrent sessions");
        }

        let id = Self::session_id();
        let now = self.now_ms();
        let record = SessionRecord {
            id: id.clone(),
            device: device_name.to_string(),
            phase: SessionPhase::Active,
            started_at: now,
            ended_at: None,
            last_fix_at: None,
            last_rx_at: None,
            fix_count: 0,
            acked: false,
        };

        let (bus, _) = broadcast::channel(256);
        let runtime = SessionRuntime {
            record: record.clone(),
            fixes: Vec::new(),
            seen_t: HashSet::new(),
            bus,
        };

        sessions.insert(id.clone(), runtime);
        device_active.insert(device_name.to_string(), id.clone());

        let index = self.snapshot_index_from_map(&sessions).await;
        drop(sessions);
        drop(device_active);
        let _ = self.persist.upsert_session(&index, &record).await;

        Ok(id)
    }

    async fn snapshot_index_from_map(
        &self,
        sessions: &HashMap<String, SessionRuntime>,
    ) -> HashMap<String, SessionRecord> {
        sessions
            .values()
            .map(|s| (s.record.id.clone(), s.record.clone()))
            .collect()
    }

    pub async fn snapshot_index(&self) -> HashMap<String, SessionRecord> {
        let sessions = self.sessions.read().await;
        self.snapshot_index_from_map(&sessions).await
    }

    pub async fn list_sessions(&self) -> Vec<LiveSessionSummary> {
        let sessions = self.sessions.read().await;
        let now = self.now_ms();
        sessions
            .values()
            .filter(|s| {
                s.record.phase == SessionPhase::Active
                    || (!s.record.acked
                        && s.record
                            .ended_at
                            .is_some_and(|end| now - end < SESSION_RETAIN_MS))
            })
            .map(|s| s.record.summary())
            .collect()
    }

    pub async fn get_fixes(&self, session_id: &str) -> Option<Vec<LiveFix>> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| s.fixes.clone())
    }

    pub async fn subscribe(
        &self,
        session_id: &str,
    ) -> Option<broadcast::Receiver<SseSessionEvent>> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| s.bus.subscribe())
    }

    pub async fn session_phase(&self, session_id: &str) -> Option<SessionPhase> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| s.record.phase)
    }

    pub async fn stop_session(&self, session_id: &str) -> bool {
        self.end_session(session_id).await
    }

    async fn end_session(&self, session_id: &str) -> bool {
        let mut sessions = self.sessions.write().await;
        let Some(runtime) = sessions.get_mut(session_id) else {
            return false;
        };
        if runtime.record.phase == SessionPhase::Ended {
            return true;
        }
        let now = self.now_ms();
        runtime.record.phase = SessionPhase::Ended;
        runtime.record.ended_at = Some(now);
        let device = runtime.record.device.clone();
        let record = runtime.record.clone();
        let _ = runtime.bus.send(SseSessionEvent::End {
            id: session_id.to_string(),
        });
        drop(sessions);
        let mut device_active = self.device_active.write().await;
        if device_active.get(&device) == Some(&session_id.to_string()) {
            device_active.remove(&device);
        }
        drop(device_active);
        let index = self.snapshot_index().await;
        let _ = self.persist.upsert_session(&index, &record).await;
        true
    }

    pub async fn ack_session(&self, session_id: &str) -> AckResult {
        let mut sessions = self.sessions.write().await;
        let Some(runtime) = sessions.get_mut(session_id) else {
            return AckResult::NotFound;
        };
        if runtime.record.phase == SessionPhase::Active {
            return AckResult::StillActive;
        }
        runtime.record.acked = true;
        let record = runtime.record.clone();
        drop(sessions);
        self.prune_session(session_id, &record).await;
        AckResult::Ok
    }

    pub async fn tick_idle_and_prune(&self) {
        let now = self.now_ms();
        let to_end: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions
                .values()
                .filter(|s| s.record.phase == SessionPhase::Active)
                .filter_map(|s| {
                    let last = s.record.last_rx_at.unwrap_or(s.record.started_at);
                    if now - last >= SESSION_IDLE_END_MS {
                        Some(s.record.id.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };
        for id in to_end {
            let _ = self.end_session(&id).await;
        }

        let to_prune: Vec<(String, SessionRecord)> = {
            let sessions = self.sessions.read().await;
            sessions
                .values()
                .filter(|s| s.record.phase == SessionPhase::Ended)
                .filter_map(|s| {
                    let end = s.record.ended_at.unwrap_or(now);
                    if s.record.acked || now - end >= SESSION_RETAIN_MS {
                        Some((s.record.id.clone(), s.record.clone()))
                    } else {
                        None
                    }
                })
                .collect()
        };
        for (id, record) in to_prune {
            self.prune_session(&id, &record).await;
        }
    }

    async fn prune_session(&self, session_id: &str, record: &SessionRecord) {
        let _ = self.persist.delete_session_files(session_id).await;
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        let mut device_active = self.device_active.write().await;
        if device_active.get(&record.device) == Some(&session_id.to_string()) {
            device_active.remove(&record.device);
        }
        let index = self.snapshot_index_from_map(&sessions).await;
        drop(sessions);
        drop(device_active);
        let _ = self.persist.replace_index(&index).await;
    }

    pub fn input_to_fix(input: crate::live::types::IngestFixInput) -> LiveFix {
        LiveFix {
            t: input.t,
            lat: input.lat,
            lon: input.lon,
            alt: input.alt,
            alt_ref: input.alt.map(|_| ALT_REF_WGS84_ELLIPSOID.to_string()),
            speed: input.speed,
            course: input.course,
            accuracy: input.accuracy,
        }
    }
}

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
use std::sync::atomic::{AtomicI64, Ordering};

#[cfg(test)]
pub struct TestClock(pub AtomicI64);

#[cfg(test)]
impl TestClock {
    pub fn new(start_ms: i64) -> Arc<TestClock> {
        Arc::new(TestClock(AtomicI64::new(start_ms)))
    }

    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0xf) as usize] as char);
        }
        out
    }
}
