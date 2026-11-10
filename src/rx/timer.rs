//! Deterministic Timer Service with stable IDs, generations, and immediate cancellation cleanup.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Mutex;

/// Unique identifier for a registered timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimerId(pub u64);

/// Internal scheduled timer entry.
struct ScheduledTimer {
    id: TimerId,
    generation: u32,
    deadline_nanos: u64,
    sequence: u64,
    cancelled: AtomicBool,
    action: Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>,
}

impl PartialEq for ScheduledTimer {
    fn eq(&self, other: &Self) -> bool {
        self.deadline_nanos == other.deadline_nanos && self.sequence == other.sequence
    }
}

impl Eq for ScheduledTimer {}

impl PartialOrd for ScheduledTimer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// Min-heap ordering: earlier deadline has higher priority
impl Ord for ScheduledTimer {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline_nanos
            .cmp(&self.deadline_nanos)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

/// Thread-safe Timer Queue managing deadlines and cancellation.
#[derive(Default)]
pub struct RxTimerService {
    next_timer_id: AtomicU64,
    next_sequence: AtomicU64,
    queue: Mutex<BinaryHeap<ScheduledTimer>>,
    active_count: AtomicU32,
}

impl RxTimerService {
    pub fn new() -> Self {
        Self {
            next_timer_id: AtomicU64::new(1),
            next_sequence: AtomicU64::new(1),
            queue: Mutex::new(BinaryHeap::new()),
            active_count: AtomicU32::new(0),
        }
    }

    /// Schedule an action to be executed when now >= deadline_nanos.
    pub fn schedule<F>(&self, deadline_nanos: u64, generation: u32, action: F) -> TimerId
    where
        F: FnOnce() + Send + 'static,
    {
        let id = TimerId(self.next_timer_id.fetch_add(1, AtomicOrdering::SeqCst));
        let sequence = self.next_sequence.fetch_add(1, AtomicOrdering::SeqCst);

        let entry = ScheduledTimer {
            id,
            generation,
            deadline_nanos,
            sequence,
            cancelled: AtomicBool::new(false),
            action: Mutex::new(Some(Box::new(action))),
        };

        let mut lock = self.queue.lock().unwrap();
        lock.push(entry);
        self.active_count.fetch_add(1, AtomicOrdering::SeqCst);
        id
    }

    /// Cancel a timer by ID and generation.
    /// Returns true if the timer was found, active, and successfully cancelled.
    /// Immediately drops the callback action to avoid retaining captures.
    pub fn cancel(&self, id: TimerId, generation: u32) -> bool {
        let mut lock = self.queue.lock().unwrap();
        let mut found = false;

        // Mark matching entry as cancelled and drop its action
        for entry in lock.iter() {
            if entry.id == id && entry.generation == generation {
                if !entry.cancelled.swap(true, AtomicOrdering::SeqCst) {
                    found = true;
                    let mut action_lock = entry.action.lock().unwrap();
                    let _ = action_lock.take(); // drop callback immediately
                    self.active_count.fetch_sub(1, AtomicOrdering::SeqCst);
                }
                break;
            }
        }

        // Clean up heap if more than 25% of entries are cancelled
        if found {
            let total = lock.len();
            let active = self.active_count.load(AtomicOrdering::SeqCst) as usize;
            if total > 8 && total - active > total / 4 {
                let mut valid = BinaryHeap::with_capacity(active);
                while let Some(item) = lock.pop() {
                    if !item.cancelled.load(AtomicOrdering::SeqCst) {
                        valid.push(item);
                    }
                }
                *lock = valid;
            }
        }

        found
    }

    /// Poll and extract all actions whose deadline has passed (deadline_nanos <= now_nanos).
    /// Skipped/cancelled timers are pruned during the poll.
    pub fn poll_expired(&self, now_nanos: u64) -> Vec<Box<dyn FnOnce() + Send + 'static>> {
        let mut ready_actions = Vec::new();
        let mut lock = self.queue.lock().unwrap();

        while let Some(top) = lock.peek() {
            if top.deadline_nanos <= now_nanos {
                let entry = lock.pop().unwrap();
                if !entry.cancelled.load(AtomicOrdering::SeqCst) {
                    self.active_count.fetch_sub(1, AtomicOrdering::SeqCst);
                    let mut action_lock = entry.action.lock().unwrap();
                    if let Some(action) = action_lock.take() {
                        ready_actions.push(action);
                    }
                }
            } else {
                break;
            }
        }

        ready_actions
    }

    /// Return the timestamp of the earliest pending timer, if any.
    pub fn next_deadline(&self) -> Option<u64> {
        let lock = self.queue.lock().unwrap();
        lock.peek().map(|t| t.deadline_nanos)
    }

    /// Return the number of currently active (non-cancelled) timers.
    pub fn active_count(&self) -> usize {
        self.active_count.load(AtomicOrdering::SeqCst) as usize
    }

    /// Total heap size including any pending cancelled tombstones.
    pub fn total_entries(&self) -> usize {
        let lock = self.queue.lock().unwrap();
        lock.len()
    }
}
