//! Reactive Lifecycle State Machine, Subscription, and Observer implementations.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::buffer::{BoundedQueue, BufferOptions, MeasurableItem};
use super::scheduler::ScopeJoinCoordinator;
use super::RxError;

/// Lifecycle states of a subscription as defined in RFC §16.3 and Astra's brief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionState {
    Active,
    Stopping,
    Joining,
    Releasing,
    Closed,
}

/// Final completion exit status of a subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatus<E> {
    Completed,
    Failed(RxError<E>),
    Cancelled,
}

/// Final outcome package returned when joining a subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion<E> {
    pub exit: ExitStatus<E>,
    pub items_delivered: usize,
}

/// Trait or structure representing a consumer of stream events.
pub struct Observer<T, E = String> {
    pub on_next: Box<dyn FnMut(T) -> Result<(), RxError<E>> + Send + 'static>,
    pub on_complete: Box<dyn FnOnce() + Send + 'static>,
    pub on_failure: Box<dyn FnOnce(RxError<E>) + Send + 'static>,
}

impl<T, E: 'static> Observer<T, E> {
    pub fn new<N, C, F>(on_next: N, on_complete: C, on_failure: F) -> Self
    where
        N: FnMut(T) -> Result<(), RxError<E>> + Send + 'static,
        C: FnOnce() + Send + 'static,
        F: FnOnce(RxError<E>) + Send + 'static,
    {
        Self {
            on_next: Box::new(on_next),
            on_complete: Box::new(on_complete),
            on_failure: Box::new(on_failure),
        }
    }

    /// Construct a simple next-only observer.
    pub fn from_next<N>(on_next: N) -> Self
    where
        N: FnMut(T) -> Result<(), RxError<E>> + Send + 'static,
    {
        Self {
            on_next: Box::new(on_next),
            on_complete: Box::new(|| {}),
            on_failure: Box::new(|_| {}),
        }
    }
}

/// Internal shared control block for a subscription.
pub struct SubscriptionControl<T: MeasurableItem + Send + 'static, E: Clone + Send + 'static> {
    pub scope_id: u64,
    pub generation: AtomicU32,
    pub state: Mutex<SubscriptionState>,
    pub exit_status: Mutex<Option<ExitStatus<E>>>,
    pub queue: Mutex<BoundedQueue<T>>,
    pub observer: Mutex<Option<Observer<T, E>>>,
    pub coordinator: Arc<ScopeJoinCoordinator>,
    pub items_delivered: AtomicUsize,
    pub terminal_dispatched: AtomicBool,
    pub current_executing_thread: AtomicU64,
}

impl<T: MeasurableItem + Send + 'static, E: Clone + Send + 'static> SubscriptionControl<T, E> {
    pub fn new(
        scope_id: u64,
        options: BufferOptions,
        observer: Observer<T, E>,
        coordinator: Arc<ScopeJoinCoordinator>,
    ) -> Self {
        Self {
            scope_id,
            generation: AtomicU32::new(1),
            state: Mutex::new(SubscriptionState::Active),
            exit_status: Mutex::new(None),
            queue: Mutex::new(BoundedQueue::new(options)),
            observer: Mutex::new(Some(observer)),
            coordinator,
            items_delivered: AtomicUsize::new(0),
            terminal_dispatched: AtomicBool::new(false),
            current_executing_thread: AtomicU64::new(0),
        }
    }

    /// Linearize cancellation:
    /// - Transition Active -> Stopping
    /// - Increment generation so late timer envelopes / arrivals are discarded
    /// - Close admission gate: no new callbacks will start
    /// - Record ExitStatus::Cancelled if no terminal outcome committed yet
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        if *state == SubscriptionState::Closed || *state == SubscriptionState::Releasing {
            return;
        }
        *state = SubscriptionState::Stopping;
        self.generation.fetch_add(1, Ordering::SeqCst);

        let mut exit = self.exit_status.lock().unwrap();
        if exit.is_none() {
            *exit = Some(ExitStatus::Cancelled);
        }
        self.coordinator.close_admission(self.scope_id);
    }

    /// Check if subscription is actively accepting new callbacks.
    pub fn is_active(&self) -> bool {
        let state = self.state.lock().unwrap();
        *state == SubscriptionState::Active
    }

    /// Deliver next item to observer.
    /// Inviolable invariant: No callback starts after cancel() has committed!
    pub fn deliver_next(&self, item: T) -> Result<(), RxError<E>> {
        // Check state and admission
        {
            let state = self.state.lock().unwrap();
            if *state != SubscriptionState::Active {
                return Err(RxError::Cancelled);
            }
        }

        // Set executing thread ID for SelfJoin detection
        let current_tid = thread_id_to_u64();
        self.current_executing_thread.store(current_tid, Ordering::SeqCst);

        let mut obs_guard = self.observer.lock().unwrap();
        let res = if let Some(ref mut obs) = *obs_guard {
            let next_res = (obs.on_next)(item);
            if next_res.is_ok() {
                self.items_delivered.fetch_add(1, Ordering::SeqCst);
            }
            next_res
        } else {
            Err(RxError::Cancelled)
        };

        self.current_executing_thread.store(0, Ordering::SeqCst);

        // If observer returned an error, fail the subscription
        if let Err(ref e) = res {
            self.fail(e.clone());
        }

        res
    }

    /// Commit normal stream completion.
    pub fn complete(&self) {
        let mut exit = self.exit_status.lock().unwrap();
        if exit.is_none() {
            *exit = Some(ExitStatus::Completed);
        }

        // Dispatch terminal complete callback exactly once if not cancelled
        if !self.terminal_dispatched.swap(true, Ordering::SeqCst) {
            let mut state = self.state.lock().unwrap();
            if *state == SubscriptionState::Active {
                *state = SubscriptionState::Stopping;
                let mut obs_guard = self.observer.lock().unwrap();
                if let Some(obs) = obs_guard.take() {
                    (obs.on_complete)();
                }
            }
        }
    }

    /// Commit stream failure.
    pub fn fail(&self, err: RxError<E>) {
        let mut exit = self.exit_status.lock().unwrap();
        if exit.is_none() {
            *exit = Some(ExitStatus::Failed(err.clone()));
        }

        // Dispatch terminal failure callback exactly once if not cancelled
        if !self.terminal_dispatched.swap(true, Ordering::SeqCst) {
            let mut state = self.state.lock().unwrap();
            if *state == SubscriptionState::Active {
                *state = SubscriptionState::Stopping;
                let mut obs_guard = self.observer.lock().unwrap();
                if let Some(obs) = obs_guard.take() {
                    (obs.on_failure)(err);
                }
            }
        }
    }

    /// Join and wait for all participants to exit.
    /// Returns Completion report.
    pub fn join(&self, timeout: Option<Duration>) -> Result<Completion<E>, RxError<E>> {
        // Detect self-join from within the observer's callback thread
        let current_tid = thread_id_to_u64();
        if self.current_executing_thread.load(Ordering::SeqCst) == current_tid && current_tid != 0 {
            return Err(RxError::SelfJoin);
        }

        {
            let mut state = self.state.lock().unwrap();
            if *state != SubscriptionState::Closed {
                *state = SubscriptionState::Joining;
            }
        }

        // Wait for all registered producers, timers, and callbacks to acknowledge exit
        let quiescent = self.coordinator.close_and_wait_quiescent(self.scope_id, timeout);
        if !quiescent {
            return Err(RxError::Timeout);
        }

        // Transition Joining -> Releasing -> Closed
        {
            let mut state = self.state.lock().unwrap();
            *state = SubscriptionState::Releasing;
            // Clean up queue and observer captures
            let mut q = self.queue.lock().unwrap();
            q.clear();
            let mut obs = self.observer.lock().unwrap();
            *obs = None;
            *state = SubscriptionState::Closed;
        }

        let exit = self
            .exit_status
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(ExitStatus::Completed);
        let delivered = self.items_delivered.load(Ordering::SeqCst);

        Ok(Completion {
            exit,
            items_delivered: delivered,
        })
    }
}

use std::sync::atomic::AtomicUsize;

/// Opaque subscription handle held by caller.
#[derive(Clone)]
pub struct Subscription<T: MeasurableItem + Send + 'static, E: Clone + Send + 'static> {
    pub control: Arc<SubscriptionControl<T, E>>,
}

impl<T: MeasurableItem + Send + 'static, E: Clone + Send + 'static> Subscription<T, E> {
    pub fn new(control: Arc<SubscriptionControl<T, E>>) -> Self {
        Self { control }
    }

    /// Cancel the subscription.
    pub fn cancel(&self) {
        self.control.cancel();
    }

    /// Join subscription, waiting for all participants to exit and resources to be released.
    pub fn join(&self, timeout: Option<Duration>) -> Result<Completion<E>, RxError<E>> {
        self.control.join(timeout)
    }

    /// Combined cancel and join.
    pub fn close(&self, timeout: Option<Duration>) -> Result<Completion<E>, RxError<E>> {
        self.cancel();
        self.join(timeout)
    }

    /// Current subscription state.
    pub fn state(&self) -> SubscriptionState {
        *self.control.state.lock().unwrap()
    }
}

fn thread_id_to_u64() -> u64 {
    let tid = std::thread::current().id();
    // Hash ThreadId to stable u64
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tid.hash(&mut hasher);
    hasher.finish()
}
