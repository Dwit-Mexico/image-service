//! Rate limit en memoria para `/admin/login` (5 intentos por IP cada 5 min).
//! Simple — no es distributed; cada pod cuenta por separado. Suficiente
//! para frenar bruteforce ingenuo. Para un atacante serio, capas de red
//! (Cloudflare, Gateway rate limit) hacen mejor trabajo.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moka::sync::Cache;

const MAX_ATTEMPTS: u32 = 5;
const WINDOW: Duration = Duration::from_secs(5 * 60);

struct Bucket {
    count: u32,
    reset_at: Instant,
}

pub struct LoginRateLimit {
    buckets: Cache<String, Arc<Mutex<Bucket>>>,
}

impl LoginRateLimit {
    pub fn new() -> Self {
        Self {
            buckets: Cache::builder()
                .max_capacity(10_000)
                .time_to_idle(WINDOW * 2)
                .build(),
        }
    }

    /// Intenta consumir 1 token. `true` si pasa, `false` si está rate limited.
    pub fn try_acquire(&self, ip: &str) -> bool {
        let now = Instant::now();
        let bucket = self.buckets.get_with(ip.to_string(), || {
            Arc::new(Mutex::new(Bucket {
                count: 0,
                reset_at: now + WINDOW,
            }))
        });
        let mut b = bucket.lock().unwrap();
        if now >= b.reset_at {
            b.count = 0;
            b.reset_at = now + WINDOW;
        }
        if b.count >= MAX_ATTEMPTS {
            return false;
        }
        b.count += 1;
        true
    }
}
