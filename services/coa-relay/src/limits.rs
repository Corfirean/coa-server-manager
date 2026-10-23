//! Relay limits, timeouts and safety bounds (Phase 13.1 Hardened).

use std::time::Duration;

pub const DEFAULT_PORT_MIN: u16 = 40000;
pub const DEFAULT_PORT_MAX: u16 = 43999; // 4,000 dynamic game ports!

pub const ALLOCATION_TIMEOUT: Duration = Duration::from_secs(120); // 2 minutes (short expiry)
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub const MAX_HOSTS: usize = 2048;
pub const MAX_ALLOCATIONS_TOTAL: usize = 4096;
pub const MAX_STREAMS_PER_REALM: usize = 2048;
pub const MAX_ALLOCATIONS_PER_REALM: usize = 2048;
pub const MAX_ALLOCATIONS_PER_PLAYER: usize = 4;

/// Maximum buffer chunk read at once (16 KiB).
pub const STREAM_CHUNK_SIZE: usize = 16 * 1024;

/// Bounded mpsc channel capacity per stream.
pub const STREAM_CHANNEL_CAPACITY: usize = 64;
