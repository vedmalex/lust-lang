//! Monotonic Clock abstraction for Rx scheduling (Virtual & Physical).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Abstract monotonic clock providing time in nanoseconds.
pub trait RxClock: Send + Sync {
    /// Return the current monotonic timestamp in nanoseconds.
    fn now_nanos(&self) -> u64;

    /// Return true if this clock is simulated/virtual.
    fn is_virtual(&self) -> bool;

    /// Advance time by the given duration (no-op for physical clocks).
    fn advance(&self, duration: Duration);
}

/// A deterministic virtual clock for reproducible testing and virtual deadlines.
#[derive(Debug, Default)]
pub struct VirtualClock {
    current_nanos: AtomicU64,
}

impl VirtualClock {
    pub fn new(initial_nanos: u64) -> Self {
        Self {
            current_nanos: AtomicU64::new(initial_nanos),
        }
    }

    pub fn set_nanos(&self, nanos: u64) {
        self.current_nanos.store(nanos, Ordering::SeqCst);
    }
}

impl RxClock for VirtualClock {
    fn now_nanos(&self) -> u64 {
        self.current_nanos.load(Ordering::SeqCst)
    }

    fn is_virtual(&self) -> bool {
        true
    }

    fn advance(&self, duration: Duration) {
        let nanos = duration.as_nanos().min(u64::MAX as u128) as u64;
        self.current_nanos.fetch_add(nanos, Ordering::SeqCst);
    }
}

/// Real physical system monotonic clock based on std::time::Instant.
pub struct PhysicalClock {
    start_instant: Instant,
}

impl PhysicalClock {
    pub fn new() -> Self {
        Self {
            start_instant: Instant::now(),
        }
    }
}

impl Default for PhysicalClock {
    fn default() -> Self {
        Self::new()
    }
}

impl RxClock for PhysicalClock {
    fn now_nanos(&self) -> u64 {
        let elapsed = self.start_instant.elapsed();
        elapsed.as_nanos().min(u64::MAX as u128) as u64
    }

    fn is_virtual(&self) -> bool {
        false
    }

    fn advance(&self, _duration: Duration) {
        // Physical clock cannot be manually advanced
    }
}

/// Convenience handle for a shared clock instance.
pub type SharedClock = Arc<dyn RxClock>;
