//! Append-only NDJSON fix log plus session index on disk under `{data}/live/`.

use crate::live::types::{LiveFix, LiveSessionSummary, SessionPhase};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    sync::Mutex,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub device: String,
    pub phase: SessionPhase,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub last_fix_at: Option<i64>,
    #[serde(default)]
    pub last_rx_at: Option<i64>,
    pub fix_count: usize,
    pub acked: bool,
}

impl SessionRecord {
    pub fn summary(&self) -> LiveSessionSummary {
        LiveSessionSummary {
            id: self.id.clone(),
            device: self.device.clone(),
            phase: self.phase,
            started_at: self.started_at,
            ended_at: self.ended_at,
            last_fix_at: self.last_fix_at,
            fix_count: self.fix_count,
        }
    }
}

pub struct LivePersist {
    root: PathBuf,
    index_lock: Mutex<()>,
}

impl LivePersist {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("live"),
            index_lock: Mutex::new(()),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("sessions.json")
    }

    fn broken_index_path(&self) -> PathBuf {
        self.root.join("sessions.json.broken")
    }

    fn log_path(&self, session_id: &str) -> PathBuf {
        self.root.join(format!("{session_id}.ndjson"))
    }

    pub async fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.root).await
    }

    pub async fn archive_corrupt_index(&self) -> std::io::Result<()> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(());
        }
        let broken = self.broken_index_path();
        if broken.exists() {
            // Keep prior broken hints; the corrupt file is not usable.
            return fs::remove_file(&path).await;
        }
        fs::rename(&path, &broken).await
    }

    pub async fn load_index(&self) -> std::io::Result<HashMap<String, SessionRecord>> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(&path).await?;
        if raw.trim().is_empty() {
            return Ok(HashMap::new());
        }
        let list: Vec<SessionRecord> = serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(list.into_iter().map(|r| (r.id.clone(), r)).collect())
    }

    pub async fn device_hints_from_broken_index(&self) -> HashMap<String, String> {
        let path = self.broken_index_path();
        let Ok(raw) = fs::read_to_string(&path).await else {
            return HashMap::new();
        };
        if raw.trim().is_empty() {
            return HashMap::new();
        }
        let list: Vec<SessionRecord> = match serde_json::from_str(&raw) {
            Ok(list) => list,
            Err(_) => return HashMap::new(),
        };
        list.into_iter().map(|r| (r.id, r.device)).collect()
    }

    pub async fn list_orphan_log_ids(
        &self,
        known: &HashMap<String, SessionRecord>,
    ) -> std::io::Result<Vec<String>> {
        let known: HashSet<&str> = known.keys().map(String::as_str).collect();
        let Ok(mut dir) = fs::read_dir(&self.root).await else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".ndjson") {
                continue;
            }
            let id = name.trim_end_matches(".ndjson").to_string();
            if !known.contains(id.as_str()) {
                out.push(id);
            }
        }
        out.sort();
        Ok(out)
    }

    async fn save_index(&self, sessions: &HashMap<String, SessionRecord>) -> std::io::Result<()> {
        let _g = self.index_lock.lock().await;
        self.ensure_dir().await?;
        let mut list: Vec<&SessionRecord> = sessions.values().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        let json = serde_json::to_string_pretty(&list)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let path = self.index_path();
        let tmp = self.root.join("sessions.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .await?;
        file.write_all(json.as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(&tmp, &path).await?;
        sync_dir_best_effort(&self.root);
        Ok(())
    }

    pub async fn upsert_session(
        &self,
        sessions: &HashMap<String, SessionRecord>,
        record: &SessionRecord,
    ) -> std::io::Result<()> {
        let mut map = sessions.clone();
        map.insert(record.id.clone(), record.clone());
        self.save_index(&map).await
    }

    pub async fn replace_index(
        &self,
        sessions: &HashMap<String, SessionRecord>,
    ) -> std::io::Result<()> {
        self.save_index(sessions).await
    }

    pub async fn append_fix(&self, session_id: &str, fix: &LiveFix) -> std::io::Result<()> {
        self.ensure_dir().await?;
        let path = self.log_path(session_id);
        let line = serde_json::to_string(fix)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        file.sync_data().await
    }

    pub async fn read_fixes(&self, session_id: &str) -> std::io::Result<Vec<LiveFix>> {
        let path = self.log_path(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&path).await?;
        let mut fixes = Vec::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(fix) = serde_json::from_str::<LiveFix>(line) {
                fixes.push(fix);
            }
        }
        fixes.sort_by_key(|f| f.t);
        Ok(fixes)
    }

    pub async fn delete_session_files(&self, session_id: &str) -> std::io::Result<()> {
        let path = self.log_path(session_id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn write_raw_index(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.ensure_dir().await?;
        fs::write(self.index_path(), bytes).await
    }

    #[cfg(test)]
    pub async fn write_fix_line(&self, session_id: &str, fix: &LiveFix) -> std::io::Result<()> {
        self.append_fix(session_id, fix).await
    }
}

fn sync_dir_best_effort(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(open) = std::fs::File::open(dir) {
            let _ = open.sync_all();
        }
    }
    let _ = dir;
}
