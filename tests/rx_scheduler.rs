//! Integration tests for Rx generic scheduler, clock, timers, and bounded backpressure queue.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lust::rx::{
    BoundedQueue, BufferOptions, EnqueueResult, OverflowStrategy, RxClock,
    RxScheduler, RxTimerService, ScopeJoinCoordinator, VirtualClock,
};

#[test]
fn test_rx_virtual_clock_monotonic_advancement() {
    let clock = VirtualClock::new(100);
    assert_eq!(clock.now_nanos(), 100);
    assert!(clock.is_virtual());

    clock.advance(Duration::from_nanos(50));
    assert_eq!(clock.now_nanos(), 150);

    clock.advance(Duration::from_millis(2));
    assert_eq!(clock.now_nanos(), 2_000_150);
}

#[test]
fn test_rx_timer_service_ordering_and_immediate_cancellation() {
    let timers = RxTimerService::new();
    let counter = Arc::new(AtomicUsize::new(0));

    // Schedule 3 timers: at t=300, t=100, t=200
    let c1 = counter.clone();
    let id1 = timers.schedule(300, 1, move || {
        c1.fetch_add(3, Ordering::SeqCst);
    });

    let c2 = counter.clone();
    let id2 = timers.schedule(100, 1, move || {
        c2.fetch_add(1, Ordering::SeqCst);
    });

    let c3 = counter.clone();
    let _id3 = timers.schedule(200, 1, move || {
        c3.fetch_add(2, Ordering::SeqCst);
    });

    assert_eq!(timers.active_count(), 3);
    assert_eq!(timers.next_deadline(), Some(100));

    // Cancel timer 1 (at t=300) with matching generation
    assert!(timers.cancel(id1, 1));
    assert_eq!(timers.active_count(), 2);

    // Cancel with wrong generation must fail and not affect active count
    assert!(!timers.cancel(id2, 999));
    assert_eq!(timers.active_count(), 2);

    // At t=50, nothing is expired
    let expired_50 = timers.poll_expired(50);
    assert_eq!(expired_50.len(), 0);

    // At t=150, timer 2 (t=100) expires
    let expired_150 = timers.poll_expired(150);
    assert_eq!(expired_150.len(), 1);
    for act in expired_150 {
        act();
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(timers.active_count(), 1);

    // At t=400, timer 3 (t=200) expires; timer 1 was cancelled so it never fires
    let expired_400 = timers.poll_expired(400);
    assert_eq!(expired_400.len(), 1);
    for act in expired_400 {
        act();
    }
    assert_eq!(counter.load(Ordering::SeqCst), 3); // 1 + 2 = 3
    assert_eq!(timers.active_count(), 0);
}

#[test]
fn test_rx_buffer_options_validation() {
    // Valid options
    let valid = BufferOptions::new(10, 100, 2, 20, OverflowStrategy::Fail);
    assert!(valid.is_ok());

    // Zero max_items rejected
    assert!(BufferOptions::new(0, 100, 0, 20, OverflowStrategy::Fail).is_err());

    // Zero max_bytes rejected
    assert!(BufferOptions::new(10, 0, 2, 0, OverflowStrategy::Fail).is_err());

    // low_items >= max_items rejected
    assert!(BufferOptions::new(10, 100, 10, 20, OverflowStrategy::Fail).is_err());
    assert!(BufferOptions::new(10, 100, 15, 20, OverflowStrategy::Fail).is_err());

    // low_bytes >= max_bytes rejected
    assert!(BufferOptions::new(10, 100, 2, 100, OverflowStrategy::Fail).is_err());
}

#[test]
fn test_rx_bounded_queue_dual_limits_and_fail_strategy() {
    // Queue max: 3 items, 24 bytes (each i64 = 8 bytes)
    let opts = BufferOptions::new(3, 24, 1, 8, OverflowStrategy::Fail).unwrap();
    let mut q: BoundedQueue<i64> = BoundedQueue::new(opts);

    // Add 1st item (8 bytes) -> OK
    assert_eq!(q.try_enqueue(10), EnqueueResult::Enqueued);
    assert_eq!(q.len(), 1);
    assert_eq!(q.charged_bytes(), 8);

    // Add 2nd item (8 bytes) -> OK (16 bytes total)
    assert_eq!(q.try_enqueue(20), EnqueueResult::Enqueued);
    assert_eq!(q.len(), 2);
    assert_eq!(q.charged_bytes(), 16);

    // Add 3rd item (8 bytes) -> OK (24 bytes total, both items=3 and bytes=24 at capacity)
    assert_eq!(q.try_enqueue(30), EnqueueResult::Enqueued);
    assert_eq!(q.len(), 3);
    assert_eq!(q.charged_bytes(), 24);

    // 4th item would exceed items and bytes -> OverflowError (fail-closed)
    assert_eq!(q.try_enqueue(40), EnqueueResult::OverflowError);
    assert_eq!(q.len(), 3);
    assert_eq!(q.charged_bytes(), 24);

    // Dequeue one item
    assert_eq!(q.dequeue(), Some(10));
    assert_eq!(q.len(), 2);
    assert_eq!(q.charged_bytes(), 16);
    assert!(!q.is_below_low_watermark()); // low is 1 item and 8 bytes

    // Dequeue another item -> now at low watermark (1 item, 8 bytes)
    assert_eq!(q.dequeue(), Some(20));
    assert_eq!(q.len(), 1);
    assert_eq!(q.charged_bytes(), 8);
    assert!(q.is_below_low_watermark());

    // Now we can enqueue again
    assert_eq!(q.try_enqueue(50), EnqueueResult::Enqueued);
    assert_eq!(q.len(), 2);
}

#[test]
fn test_rx_bounded_queue_drop_newest_and_drop_oldest() {
    // DropNewest
    let opts_newest = BufferOptions::new(2, 64, 1, 16, OverflowStrategy::DropNewest).unwrap();
    let mut q_newest: BoundedQueue<i64> = BoundedQueue::new(opts_newest);

    assert_eq!(q_newest.try_enqueue(1), EnqueueResult::Enqueued);
    assert_eq!(q_newest.try_enqueue(2), EnqueueResult::Enqueued);
    // 3rd item dropped
    assert_eq!(q_newest.try_enqueue(3), EnqueueResult::DroppedNewest);
    assert_eq!(q_newest.dropped_count(), 1);
    assert_eq!(q_newest.dequeue(), Some(1));
    assert_eq!(q_newest.dequeue(), Some(2));
    assert_eq!(q_newest.dequeue(), None);

    // DropOldest
    let opts_oldest = BufferOptions::new(2, 64, 1, 16, OverflowStrategy::DropOldest).unwrap();
    let mut q_oldest: BoundedQueue<i64> = BoundedQueue::new(opts_oldest);

    assert_eq!(q_oldest.try_enqueue(1), EnqueueResult::Enqueued);
    assert_eq!(q_oldest.try_enqueue(2), EnqueueResult::Enqueued);
    // 3rd item replaces oldest (1)
    assert_eq!(q_oldest.try_enqueue(3), EnqueueResult::ReplacedOldest(1));
    assert_eq!(q_oldest.dropped_count(), 1);
    assert_eq!(q_oldest.dequeue(), Some(2));
    assert_eq!(q_oldest.dequeue(), Some(3));
    assert_eq!(q_oldest.dequeue(), None);
}

#[test]
fn test_rx_scope_join_coordinator_quiescence() {
    let coordinator = Arc::new(ScopeJoinCoordinator::new());
    let scope_id = 42;

    assert_eq!(coordinator.active_participants(scope_id), 0);
    assert!(coordinator.wait_for_quiescence(scope_id, Some(Duration::from_millis(10))));

    // Register two participants
    let p1 = coordinator.register_participant(scope_id);
    let p2 = coordinator.register_participant(scope_id);
    assert_eq!(coordinator.active_participants(scope_id), 2);

    // Timeout while participants are active
    assert!(!coordinator.wait_for_quiescence(scope_id, Some(Duration::from_millis(10))));

    // P1 exits
    coordinator.acknowledge_exit(scope_id, p1);
    assert_eq!(coordinator.active_participants(scope_id), 1);
    assert!(!coordinator.wait_for_quiescence(scope_id, Some(Duration::from_millis(10))));

    // Spawn thread that acknowledges P2 after 20ms
    let coord_clone = coordinator.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        coord_clone.acknowledge_exit(scope_id, p2);
    });

    // Blocking wait until quiescent succeeds
    assert!(coordinator.wait_for_quiescence(scope_id, Some(Duration::from_secs(1))));
    assert_eq!(coordinator.active_participants(scope_id), 0);
}

#[test]
fn test_rx_scheduler_task_execution_and_virtual_time_advancement() {
    let scheduler = RxScheduler::new_virtual();
    let steps_counter = Arc::new(AtomicUsize::new(0));

    // Schedule task at t=0
    let s1 = steps_counter.clone();
    scheduler.schedule_task(1, move || {
        s1.fetch_add(10, Ordering::SeqCst);
    });

    // Schedule timer for t=5000 nanos
    let s2 = steps_counter.clone();
    scheduler
        .timers
        .schedule(5000, 1, move || {
            s2.fetch_add(20, Ordering::SeqCst);
        });

    assert_eq!(scheduler.clock.now_nanos(), 0);

    // Run until quiescent: executes immediate task, then advances clock to 5000 and executes timer task
    let total_steps = scheduler.run_until_quiescent(100);
    assert_eq!(total_steps, 2);
    assert_eq!(steps_counter.load(Ordering::SeqCst), 30); // 10 + 20
    assert_eq!(scheduler.clock.now_nanos(), 5000);
}
