//! Observable blueprint and stream creation factories.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::buffer::{BufferOptions, MeasurableItem};
use super::lifecycle::{Observer, Subscription, SubscriptionControl};
use super::scheduler::RxScheduler;
use super::RxError;

/// Type alias for stream producer closure.
pub type ProducerFn<T, E> =
    Box<dyn FnOnce(Arc<SubscriptionControl<T, E>>) -> Result<(), RxError<E>> + Send + 'static>;

/// A cold, lazy blueprint for a reactive stream.
/// Executed only upon call to `subscribe`.
pub struct Observable<T: MeasurableItem + Send + 'static, E: Clone + Send + 'static = String> {
    producer: ProducerFn<T, E>,
}

impl<T: MeasurableItem + Send + 'static, E: Clone + Send + 'static> Observable<T, E> {
    pub fn new<F>(producer: F) -> Self
    where
        F: FnOnce(Arc<SubscriptionControl<T, E>>) -> Result<(), RxError<E>> + Send + 'static,
    {
        Self {
            producer: Box::new(producer),
        }
    }

    /// Create an Observable from an in-memory iterable collection.
    /// Pulls items lazily on demand until collection ends or subscription cancels.
    pub fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T> + Send + 'static,
        I::IntoIter: Send + 'static,
    {
        Self::new(move |control| {
            let mut iterator = iter.into_iter();
            while control.is_active() {
                if let Some(item) = iterator.next() {
                    if let Err(e) = control.deliver_next(item) {
                        return Err(e);
                    }
                } else {
                    control.complete();
                    break;
                }
            }
            Ok(())
        })
    }

    /// Create an Observable driven by an arbitrary cooperative producer closure.
    pub fn from_producer<F>(producer_fn: F) -> Self
    where
        F: FnOnce(Arc<SubscriptionControl<T, E>>) -> Result<(), RxError<E>> + Send + 'static,
    {
        Self::new(producer_fn)
    }

    /// Subscribe an observer to this Observable within a given scope.
    pub fn subscribe(
        self,
        observer: Observer<T, E>,
        scope_id: u64,
        options: BufferOptions,
        scheduler: Arc<RxScheduler>,
    ) -> Result<Subscription<T, E>, RxError<E>> {
        // Register subscription with coordinator
        let participant_id = scheduler
            .coordinator
            .register_participant(scope_id)
            .map_err(|_| RxError::Cancelled)?;

        let control = Arc::new(SubscriptionControl::new(
            scope_id,
            options,
            observer,
            scheduler.coordinator.clone(),
        ));

        let subscription = Subscription::new(control.clone());

        // Spawn or run producer task
        let ctrl_clone = control.clone();
        let producer = self.producer;

        scheduler
            .schedule_task(10, move || {
                let res = producer(ctrl_clone.clone());
                if let Err(e) = res {
                    ctrl_clone.fail(e);
                }
                ctrl_clone
                    .coordinator
                    .acknowledge_exit(scope_id, participant_id);
            })
            .map_err(|_| RxError::BackpressureOverflow)?;

        Ok(subscription)
    }
}

/// Standalone timer interval stream factory.
pub fn interval<E: Clone + Send + 'static>(
    period: Duration,
    scheduler: Arc<RxScheduler>,
) -> Observable<i64, E> {
    Observable::<i64, E>::new(move |control| {
        let period_nanos = period.as_nanos().min(u64::MAX as u128) as u64;
        let counter = Arc::new(std::sync::atomic::AtomicI64::new(0));

        let now = scheduler.clock.now_nanos();
        let first_deadline = now + period_nanos;

        let ctrl_clone = control.clone();
        let sched_clone = scheduler.clone();
        let cnt_clone = counter.clone();

        let participant_id = match control.coordinator.register_participant(control.scope_id) {
            Ok(p) => p,
            Err(_) => return Err(RxError::Cancelled),
        };

        // Recursive tick scheduling closure
        struct TickScheduler<E: Clone + Send + 'static> {
            control: Arc<SubscriptionControl<i64, E>>,
            scheduler: Arc<RxScheduler>,
            counter: Arc<std::sync::atomic::AtomicI64>,
            period_nanos: u64,
            participant_id: super::scheduler::ParticipantId,
        }

        impl<E: Clone + Send + 'static> TickScheduler<E> {
            fn schedule_next(self: Arc<Self>, deadline: u64) {
                let self_clone = self.clone();
                self.scheduler.timers.schedule(
                    deadline,
                    self.control.generation.load(Ordering::SeqCst),
                    move || {
                        if !self_clone.control.is_active() {
                            self_clone.control.coordinator.acknowledge_exit(
                                self_clone.control.scope_id,
                                self_clone.participant_id,
                            );
                            return;
                        }

                        let tick_val = self_clone.counter.fetch_add(1, Ordering::SeqCst);
                        if let Ok(()) = self_clone.control.deliver_next(tick_val) {
                            let next_deadline = deadline + self_clone.period_nanos;
                            self_clone.schedule_next(next_deadline);
                        } else {
                            self_clone.control.coordinator.acknowledge_exit(
                                self_clone.control.scope_id,
                                self_clone.participant_id,
                            );
                        }
                    },
                );
            }
        }

        let ticker = Arc::new(TickScheduler {
            control: ctrl_clone,
            scheduler: sched_clone,
            counter: cnt_clone,
            period_nanos,
            participant_id,
        });

        ticker.schedule_next(first_deadline);
        Ok(())
    })
}
