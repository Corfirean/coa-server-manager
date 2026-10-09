//! Dynamic port pool management for game relay listeners.

use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Debug)]
pub struct PortPool {
    min: u16,
    max: u16,
    used: Mutex<HashSet<u16>>,
}

impl PortPool {
    pub fn new(min: u16, max: u16) -> Self {
        assert!(min < max, "min port must be less than max port");
        Self {
            min,
            max,
            used: Mutex::new(HashSet::new()),
        }
    }

    /// Allocates an unused port, if any is available in the pool range.
    pub fn acquire(&self) -> Option<u16> {
        let mut used = self.used.lock().unwrap();
        for p in self.min..=self.max {
            if !used.contains(&p) {
                used.insert(p);
                return Some(p);
            }
        }
        None
    }

    /// Acquires a pair of ports: (auth_port, world_port).
    pub fn acquire_pair(&self) -> Option<(u16, u16)> {
        let mut used = self.used.lock().unwrap();
        let mut first = None;
        for p in self.min..=self.max {
            if !used.contains(&p) {
                if first.is_none() {
                    first = Some(p);
                } else {
                    let a = first.unwrap();
                    used.insert(a);
                    used.insert(p);
                    return Some((a, p));
                }
            }
        }
        None
    }

    /// Releases a port back to the pool.
    pub fn release(&self, port: u16) {
        let mut used = self.used.lock().unwrap();
        used.remove(&port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_pool() {
        let pool = PortPool::new(40000, 40003);
        let pair = pool.acquire_pair().unwrap();
        assert_eq!(pair, (40000, 40001));

        let next = pool.acquire().unwrap();
        assert_eq!(next, 40002);

        let last = pool.acquire().unwrap();
        assert_eq!(last, 40003);

        assert!(pool.acquire().is_none());

        pool.release(40001);
        assert_eq!(pool.acquire(), Some(40001));
    }
}
