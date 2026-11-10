//! RxScheduler coordinating virtual/physical execution, task queues, and participant join tracking.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::clock::{SharedClock, VirtualClock};
use super::timer::RxTimerService;
use super::RxError;

/// Identifier for tracking active participants of a subscription scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParticipantId(pub u64);

/// Tracks active participants per scope to guarantee zero lingering callbacks on join.
#[derive(Default)]
pub struct ScopeJoinCoordinator {
    next_id: AtomicU64,
    // scope_id -> set of active participants
    participants: Mutex<HashMap<u64, Vec<ParticipantId>>>,
    closed_scopes: Mutex<HashSet<u64>>,
    condvar: Condvar,
}

impl ScopeJoinCoordinator {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            participants: Mutex::new(HashMap::new()),
            closed_scopes: Mutex::new(HashSet::new()),
            condvar: Condvar::new(),
        }
    }

    /// Register a new active participant for a given scope.
    /// Fails closed if admission has already been closed for this scope.
    pub fn register_participant(&self, scope_id: u64) -> Result<ParticipantId, RxError> {
        let closed = self.closed_scopes.lock().unwrap();
        if closed.contains(&scope_id) {
            return Err(RxError::Cancelled);
        }
        let id = ParticipantId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let mut map = self.participants.lock().unwrap();
        map.entry(scope_id).or_default().push(id);
        Ok(id)
    }

    /// Close admission for a scope so that no new participants can register.
    pub fn close_admission(&self, scope_id: u64) {
        let mut closed = self.closed_scopes.lock().unwrap();
        closed.insert(scope_id);
    }

    /// Atomically close admission and wait until all registered participants have exited.
    pub fn close_and_wait_quiescent(&self, scope_id: u64, timeout: Option<Duration>) -> bool {
        self.close_admission(scope_id);
        self.wait_for_quiescence(scope_id, timeout)
    }

    /// Acknowledge that a participant has completely exited.
    pub fn acknowledge_exit(&self, scope_id: u64, participant_id: ParticipantId) {
        let mut map = self.participants.lock().unwrap();
        if let Some(list) = map.get_mut(&scope_id) {
            list.retain(|&p| p != participant_id);
            if list.is_empty() {
                map.remove(&scope_id);
            }
        }
        self.condvar.notify_all();
    }

    /// Return count of currently active participants for a scope.
    pub fn active_participants(&self, scope_id: u64) -> usize {
        let map = self.participants.lock().unwrap();
        map.get(&scope_id).map_or(0, |list| list.len())
    }

    /// Check if admission has been closed for a scope.
    pub fn is_admission_closed(&self, scope_id: u64) -> bool {
        let closed = self.closed_scopes.lock().unwrap();
        closed.contains(&scope_id)
    }

    /// Block or wait until all participants for the given scope have exited.
    pub fn wait_for_quiescence(&self, scope_id: u64, timeout: Option<Duration>) -> bool {
        let mut map = self.participants.lock().unwrap();
        let start = std::time::Instant::now();

        loop {
            let active = map.get(&scope_id).map_or(0, |l| l.len());
            if active == 0 {
                return true;
            }

            if let Some(limit) = timeout {
                let elapsed = start.elapsed();
                if elapsed >= limit {
                    return false;
                }
                let remaining = limit - elapsed;
                let (new_map, wait_res) = self.condvar.wait_timeout(map, remaining).unwrap();
                map = new_map;
                if wait_res.timed_out() {
                    return map.get(&scope_id).map_or(0, |l| l.len()) == 0;
                }
            } else {
                map = self.condvar.wait(map).unwrap();
            }
        }
    }
}

/// A scheduled task entry in the ready queue.
pub struct ReadyTask {
    pub sequence: u64,
    pub priority: u8,
    pub task: Box<dyn FnOnce() + Send + 'static>,
}

/// Core Scheduler coordinating clocks, timers, and cooperative task loops.
pub struct RxScheduler {
    pub clock: SharedClock,
    pub timers: Arc<RxTimerService>,
    pub coordinator: Arc<ScopeJoinCoordinator>,
    ready_tasks: Mutex<VecDeque<ReadyTask>>,
    max_ready_tasks: usize,
    next_task_seq: AtomicU64,
    stopped: AtomicBool,
}

impl RxScheduler {
    pub fn new_virtual() -> Self {
        Self::new_virtual_with_capacity(1024)
    }

    pub fn new_virtual_with_capacity(max_ready_tasks: usize) -> Self {
        let clock: SharedClock = Arc::new(VirtualClock::new(0));
        let timers = Arc::new(RxTimerService::new());
        let coordinator = Arc::new(ScopeJoinCoordinator::new());
        Self {
            clock,
            timers,
            coordinator,
            ready_tasks: Mutex::new(VecDeque::with_capacity(max_ready_tasks.min(4096))),
            max_ready_tasks,
            next_task_seq: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
        }
    }

    pub fn new_with_clock(clock: SharedClock, max_ready_tasks: usize) -> Self {
        let timers = Arc::new(RxTimerService::new());
        let coordinator = Arc::new(ScopeJoinCoordinator::new());
        Self {
            clock,
            timers,
            coordinator,
            ready_tasks: Mutex::new(VecDeque::with_capacity(max_ready_tasks.min(4096))),
            max_ready_tasks,
            next_task_seq: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
        }
    }

    /// Enqueue a cooperative task to run in the scheduler.
    /// Returns Err(RxError::BackpressureOverflow) if ready queue capacity is exceeded.
    pub fn schedule_task<F>(&self, priority: u8, task: F) -> Result<(), RxError>
    where
        F: FnOnce() + Send + 'static,
    {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(RxError::Cancelled);
        }
        let sequence = self.next_task_seq.fetch_add(1, Ordering::SeqCst);
        let entry = ReadyTask {
            sequence,
            priority,
            task: Box::new(task),
        };
        let mut queue = self.ready_tasks.lock().unwrap();
        if queue.len() >= self.max_ready_tasks {
            return Err(RxError::BackpressureOverflow);
        }
        queue.push_back(entry);
        Ok(())
    }

    /// Execute a single pending task if available.
    /// Also drains and enqueues any expired timers.
    pub fn step(&self) -> bool {
        // Poll expired timers
        let now = self.clock.now_nanos();
        let expired = self.timers.poll_expired(now);
        for action in expired {
            let _ = self.schedule_task(10, action);
        }

        // Take the next ready task
        let next = {
            let mut queue = self.ready_tasks.lock().unwrap();
            queue.pop_front()
        };

        if let Some(task_entry) = next {
            (task_entry.task)();
            true
        } else {
            false
        }
    }

    /// Run the scheduler until there are no remaining ready tasks and no timers can be triggered.
    /// In a virtual clock environment, if there are pending timers, advances time to the next deadline.
    pub fn run_until_quiescent(&self, max_steps: usize) -> usize {
        let mut steps = 0;
        while steps < max_steps {
            if self.step() {
                steps += 1;
                continue;
            }

            // If no ready tasks, check if virtual clock can advance to next deadline
            if self.clock.is_virtual() {
                if let Some(next_deadline) = self.timers.next_deadline() {
                    let now = self.clock.now_nanos();
                    if next_deadline > now {
                        let advance_duration = Duration::from_nanos(next_deadline - now);
                        self.clock.advance(advance_duration);
                        if self.step() {
                            steps += 1;
                            continue;
                        }
                    }
                }
            }

            // Quiescent
            break;
        }
        steps
    }

    /// Stop accepting new work.
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}
