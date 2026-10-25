//! In-memory fixed-window rate limits and the TOTP lockout.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

pub struct Limiter {
    window: Duration,
    max: u32,
    hits: Mutex<HashMap<String, (Instant, u32)>>,
}

impl Limiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self { window, max, hits: Mutex::new(HashMap::new()) }
    }

    /// Count a hit for `key`. Returns false when the key is over the limit.
    pub fn hit(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        if hits.len() > 100_000 {
            hits.retain(|_, (start, _)| now.duration_since(*start) < self.window);
        }
        let entry = hits.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max
    }

    /// True when `key` is over the limit, without counting a hit.
    pub fn is_blocked(&self, key: &str) -> bool {
        let now = Instant::now();
        let hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        hits.get(key).is_some_and(|(start, n)| now.duration_since(*start) < self.window && *n >= self.max)
    }

    pub fn reset(&self, key: &str) {
        self.hits.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
    }
}

/// All limits the server applies.
pub struct Limits {
    /// Login attempts per IP and per email: 10 a minute.
    pub login: Limiter,
    /// Wrong TOTP codes per user: 5, then locked for 15 minutes.
    pub totp: Limiter,
    /// Merchant API calls per client IP: 600 a minute.
    pub api: Limiter,
    /// Checkout page loads per client IP: 300 a minute.
    pub checkout: Limiter,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            login: Limiter::new(10, Duration::from_secs(60)),
            totp: Limiter::new(5, Duration::from_secs(15 * 60)),
            api: Limiter::new(600, Duration::from_secs(60)),
            checkout: Limiter::new(300, Duration::from_secs(60)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max() {
        let l = Limiter::new(2, Duration::from_secs(60));
        assert!(l.hit("a"));
        assert!(l.hit("a"));
        assert!(!l.hit("a"));
        assert!(l.is_blocked("a"));
        assert!(l.hit("b"));
        l.reset("a");
        assert!(l.hit("a"));
    }
}
