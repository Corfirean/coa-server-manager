//! Dynamic port pool management with O(1) randomized acquisition and O(1) release.

use std::collections::HashMap;
use std::sync::Mutex;
use rand::Rng;

#[derive(Debug)]
pub struct PortPool {
    min: u16,
    max: u16,
    inner: Mutex<PoolInner>,
}

#[derive(Debug)]
struct PoolInner {
    available: Vec<u16>,
    index_map: HashMap<u16, usize>,
}

impl PortPool {
    pub fn new(min: u16, max: u16) -> Self {
        assert!(min < max, "min port must be less than max port");
        let count = (max - min + 1) as usize;
        let mut available = Vec::with_capacity(count);
        let mut index_map = HashMap::with_capacity(count);
        for (idx, p) in (min..=max).enumerate() {
            available.push(p);
            index_map.insert(p, idx);
        }
        Self {
            min,
            max,
            inner: Mutex::new(PoolInner { available, index_map }),
        }
    }

    pub fn capacity(&self) -> usize {
        (self.max - self.min + 1) as usize
    }

    pub fn available_count(&self) -> usize {
        self.inner.lock().unwrap().available.len()
    }

    /// Allocates a single random unused port in O(1) time.
    pub fn acquire(&self) -> Option<u16> {
        let mut inner = self.inner.lock().unwrap();
        if inner.available.is_empty() {
            return None;
        }
        let rand_idx = rand::thread_rng().gen_range(0..inner.available.len());
        let port = inner.available[rand_idx];
        let last = inner.available.pop().unwrap();
        inner.index_map.remove(&port);
        if rand_idx < inner.available.len() {
            inner.available[rand_idx] = last;
            inner.index_map.insert(last, rand_idx);
        }
        Some(port)
    }

    /// Acquires a pair of random unused ports: (auth_port, world_port) in O(1) time.
    pub fn acquire_pair(&self) -> Option<(u16, u16)> {
        let mut inner = self.inner.lock().unwrap();
        if inner.available.len() < 2 {
            return None;
        }
        let mut rng = rand::thread_rng();

        // 1. Pick first port
        let idx1 = rng.gen_range(0..inner.available.len());
        let p1 = inner.available[idx1];
        let last1 = inner.available.pop().unwrap();
        inner.index_map.remove(&p1);
        if idx1 < inner.available.len() {
            inner.available[idx1] = last1;
            inner.index_map.insert(last1, idx1);
        }

        // 2. Pick second port
        let idx2 = rng.gen_range(0..inner.available.len());
        let p2 = inner.available[idx2];
        let last2 = inner.available.pop().unwrap();
        inner.index_map.remove(&p2);
        if idx2 < inner.available.len() {
            inner.available[idx2] = last2;
            inner.index_map.insert(last2, idx2);
        }

        Some((p1, p2))
    }

    /// Releases a port back to the pool in O(1) time.
    pub fn release(&self, port: u16) {
        if port < self.min || port > self.max {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if !inner.index_map.contains_key(&port) {
            let idx = inner.available.len();
            inner.available.push(port);
            inner.index_map.insert(port, idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_randomized_port_pool() {
        let pool = PortPool::new(40000, 40003);
        assert_eq!(pool.available_count(), 4);

        let (p1, p2) = pool.acquire_pair().unwrap();
        assert_ne!(p1, p2);
        assert!(p1 >= 40000 && p1 <= 40003);
        assert!(p2 >= 40000 && p2 <= 40003);
        assert_eq!(pool.available_count(), 2);

        let _p3 = pool.acquire().unwrap();
        let _p4 = pool.acquire().unwrap();
        assert_eq!(pool.available_count(), 0);
        assert!(pool.acquire().is_none());

        pool.release(p1);
        assert_eq!(pool.available_count(), 1);
        assert_eq!(pool.acquire(), Some(p1));
    }
}
