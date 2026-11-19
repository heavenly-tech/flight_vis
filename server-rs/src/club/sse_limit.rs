use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

const GLOBAL_MAX: usize = 256;
const PER_IP_MAX: usize = 8;
pub const SSE_IDLE_MS: u64 = 5 * 60 * 1000;

pub struct SseLimiter {
    global: AtomicUsize,
    per_ip: Mutex<HashMap<String, usize>>,
}

impl Default for SseLimiter {
    fn default() -> Self {
        Self {
            global: AtomicUsize::new(0),
            per_ip: Mutex::new(HashMap::new()),
        }
    }
}

pub struct SseGuard {
    limiter: std::sync::Arc<SseLimiter>,
    ip: String,
}

impl Drop for SseGuard {
    fn drop(&mut self) {
        self.limiter.global.fetch_sub(1, Ordering::SeqCst);
        let mut map = self.limiter.per_ip.lock().unwrap();
        if let Some(n) = map.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&self.ip);
            }
        }
    }
}

impl SseLimiter {
    pub fn try_admit(self: &std::sync::Arc<Self>, ip: &str) -> Option<SseGuard> {
        loop {
            let cur = self.global.load(Ordering::SeqCst);
            if cur >= GLOBAL_MAX {
                return None;
            }
            if self
                .global
                .compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                continue;
            }
            break;
        }
        {
            let mut map = self.per_ip.lock().unwrap();
            let entry = map.entry(ip.to_string()).or_insert(0);
            if *entry >= PER_IP_MAX {
                self.global.fetch_sub(1, Ordering::SeqCst);
                return None;
            }
            *entry += 1;
        }
        Some(SseGuard {
            limiter: self.clone(),
            ip: ip.to_string(),
        })
    }
}
