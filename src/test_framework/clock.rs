//! Virtual Time Engine (TestClock) for deterministic testing of timeouts, retries, and schedules.

use alloc::{collections::BinaryHeap, vec::Vec};
use core::cmp::Reverse;
use crate::effect_abi::{ScheduleKind, ScheduleState, ScheduleStep};

/// A timer scheduled in virtual time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTimer {
    pub due_time_ms: u64,
    pub fiber_id: u64,
    pub timer_id: u64,
}

impl Ord for ScheduledTimer {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.due_time_ms
            .cmp(&other.due_time_ms)
            .then_with(|| self.timer_id.cmp(&other.timer_id))
    }
}

impl PartialOrd for ScheduledTimer {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Virtual clock state managing time and queued fiber wakeups.
#[derive(Debug, Clone, Default)]
pub struct TestClockState {
    pub current_time_ms: u64,
    pub timers: BinaryHeap<Reverse<ScheduledTimer>>,
    pub next_timer_id: u64,
}

impl TestClockState {
    /// Creates a new virtual clock initialized at t = 0.
    pub fn new() -> Self {
        Self {
            current_time_ms: 0,
            timers: BinaryHeap::new(),
            next_timer_id: 1,
        }
    }

    /// Returns current virtual time in milliseconds.
    #[inline]
    pub fn now_ms(&self) -> u64 {
        self.current_time_ms
    }

    /// Schedules a fiber to be awakened after `delay_ms`.
    /// Returns the assigned unique `timer_id`.
    pub fn schedule(&mut self, fiber_id: u64, delay_ms: u64) -> u64 {
        let timer_id = self.next_timer_id;
        self.next_timer_id += 1;
        let due_time_ms = self.current_time_ms.saturating_add(delay_ms);

        self.timers.push(Reverse(ScheduledTimer {
            due_time_ms,
            fiber_id,
            timer_id,
        }));

        timer_id
    }

    /// Advances virtual time by `delta_ms`.
    /// Wakes up all timers due at or before the new time, returning their fiber IDs.
    pub fn adjust(&mut self, delta_ms: u64) -> Vec<u64> {
        self.current_time_ms = self.current_time_ms.saturating_add(delta_ms);
        let mut awakened_fibers = Vec::new();

        while let Some(Reverse(top)) = self.timers.peek() {
            if top.due_time_ms <= self.current_time_ms {
                let Reverse(timer) = self.timers.pop().unwrap();
                awakened_fibers.push(timer.fiber_id);
            } else {
                break;
            }
        }

        awakened_fibers
    }

    /// Fast-forwards virtual time to an absolute `target_ms`.
    pub fn set_time(&mut self, target_ms: u64) -> Vec<u64> {
        if target_ms <= self.current_time_ms {
            return Vec::new();
        }
        let delta = target_ms - self.current_time_ms;
        self.adjust(delta)
    }

    /// Returns the number of timers currently waiting in the queue.
    #[inline]
    pub fn pending_timer_count(&self) -> usize {
        self.timers.len()
    }

    /// Returns the delay in milliseconds until the next scheduled timer fires, if any.
    pub fn next_due_in(&self) -> Option<u64> {
        self.timers.peek().map(|Reverse(t)| {
            if t.due_time_ms <= self.current_time_ms {
                0
            } else {
                t.due_time_ms - self.current_time_ms
            }
        })
    }

    /// Simulates stepping a schedule policy in virtual time without waiting,
    /// accumulating the total delay and step count.
    pub fn simulate_schedule(
        &mut self,
        policy: &ScheduleKind,
        max_steps: usize,
    ) -> (usize, u64) {
        let mut steps = 0;
        let mut total_delay_ms = 0u64;
        let mut state = ScheduleState::initial();

        while steps < max_steps {
            match policy.step(&state) {
                ScheduleStep::Continue { delay_ms, next_state } => {
                    steps += 1;
                    total_delay_ms = total_delay_ms.saturating_add(delay_ms);
                    self.adjust(delay_ms);
                    state = next_state;
                }
                ScheduleStep::Done => break,
            }
        }

        (steps, total_delay_ms)
    }
}
