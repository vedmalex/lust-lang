//! Reactive Streams (Rx) Foundation for Lust.
//! Implements scoped cancellation, backpressure, virtual scheduling, and deterministic timers.

pub mod buffer;
pub mod clock;
pub mod scheduler;
pub mod timer;

pub use buffer::{BoundedQueue, BufferOptions, EnqueueResult, MeasurableItem, OverflowStrategy};
pub use clock::{PhysicalClock, RxClock, SharedClock, VirtualClock};
pub use scheduler::{ParticipantId, ReadyTask, RxScheduler, ScopeJoinCoordinator};
pub use timer::{RxTimerService, TimerId};

/// Core error types for the Reactive Streams subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RxError<E = String> {
    /// Upstream source failed with a user error.
    Source(E),
    /// Queue capacity reached and overflow strategy is Fail.
    BackpressureOverflow,
    /// Single item exceeds total byte capacity of buffer.
    ItemTooLarge,
    /// Invalid configuration options (e.g., non-positive capacity or inverted watermarks).
    InvalidOptions(String),
    /// Subscription handle was invalidated, stale, or from an unauthorized domain.
    InvalidHandle,
    /// Attempted self-join from within an active callback.
    SelfJoin,
    /// Operation timed out.
    Timeout,
    /// Subscription cancelled by scope or downstream observer.
    Cancelled,
}

impl<E: std::fmt::Display> std::fmt::Display for RxError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(e) => write!(f, "Rx source error: {}", e),
            Self::BackpressureOverflow => write!(f, "Rx backpressure overflow"),
            Self::ItemTooLarge => write!(f, "Rx item exceeds buffer byte capacity"),
            Self::InvalidOptions(msg) => write!(f, "Invalid Rx buffer options: {}", msg),
            Self::InvalidHandle => write!(f, "Invalid or stale Rx subscription handle"),
            Self::SelfJoin => write!(f, "Deadlock prevented: cannot join subscription from its own callback"),
            Self::Timeout => write!(f, "Rx operation timed out"),
            Self::Cancelled => write!(f, "Rx stream subscription was cancelled"),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for RxError<E> {}
