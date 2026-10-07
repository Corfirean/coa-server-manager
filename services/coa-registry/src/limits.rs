//! In-memory token buckets for abuse control. One process, one node: no shared store is needed.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug)]
pub struct Rate {
    pub burst: f64,
    /// Tokens per second.
    pub refill: f64,
}

impl Rate {
    pub fn per_minute(burst: u32, per_minute: f64) -> Self {
        Self { burst: burst as f64, refill: per_minute / 60.0 }
    }
    pub fn per_hour(burst: u32, per_hour: f64) -> Self {
        Self { burst: burst as f64, refill: per_hour / 3600.0 }
    }
}

struct Bucket {
    tokens: f64,
    at: f64,
}

pub struct Limiter<K: Eq + Hash + Clone> {
    rate: Rate,
    max_keys: usize,
    buckets: Mutex<HashMap<K, Bucket>>,
}

impl<K: Eq + Hash + Clone> Limiter<K> {
    pub fn new(rate: Rate, max_keys: usize) -> Self {
        Self { rate, max_keys, buckets: Mutex::new(HashMap::new()) }
    }

    /// Take `cost` tokens at time `now` (seconds, any monotonic origin). `false`: refused, nothing taken.
    pub fn take(&self, key: &K, cost: f64, now: f64) -> bool {
        let mut map = self.buckets.lock().unwrap();
        if map.len() >= self.max_keys && !map.contains_key(key) {
            // a full table is cleaned of buckets that have refilled completely (they carry no information)
            let full = self.rate.burst;
            let refill = self.rate.refill;
            map.retain(|_, b| b.tokens + (now - b.at).max(0.0) * refill < full);
            if map.len() >= self.max_keys {
                return false;
            }
        }
        let b = map.entry(key.clone()).or_insert(Bucket { tokens: self.rate.burst, at: now });
        b.tokens = (b.tokens + (now - b.at).max(0.0) * self.rate.refill).min(self.rate.burst);
        b.at = now;
        if b.tokens >= cost {
            b.tokens -= cost;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_allows_a_burst_then_refills() {
        let l: Limiter<&str> = Limiter::new(Rate { burst: 3.0, refill: 0.5 }, 100);
        assert!(l.take(&"a", 1.0, 0.0) && l.take(&"a", 1.0, 0.0) && l.take(&"a", 1.0, 0.0));
        assert!(!l.take(&"a", 1.0, 0.0), "the burst is used up");
        assert!(l.take(&"b", 1.0, 0.0), "another key is unaffected");
        assert!(!l.take(&"a", 1.0, 1.0), "half a token is not enough");
        assert!(l.take(&"a", 1.0, 2.0), "one token after two seconds");
        assert!(l.take(&"a", 3.0, 1000.0), "a long pause refills to the burst");
        assert!(!l.take(&"a", 1.0, 1000.0), "and never beyond it");
    }

    #[test]
    fn the_table_is_bounded() {
        let l: Limiter<u32> = Limiter::new(Rate { burst: 1.0, refill: 0.0 }, 4);
        for k in 0..4 {
            assert!(l.take(&k, 1.0, 0.0));
        }
        assert!(!l.take(&99, 1.0, 0.0), "a new key is refused when the table is full of live buckets");
        assert!(!l.take(&0, 1.0, 0.0), "an existing one keeps its state");
    }
}
