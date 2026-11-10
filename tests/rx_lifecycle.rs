//! Integration tests for Rx Lifecycle State Machine, Subscription, and Observer.
//! Verifies zero lingering callbacks post-join, self-unsubscribe, and terminal protocols.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lust::rx::{
    interval, BufferOptions, ExitStatus, Observable, Observer, RxError, RxScheduler,
    Subscription,
};

#[test]
fn test_rx_normal_completion_exactly_once() {
    let scheduler = Arc::new(RxScheduler::new_virtual());
    let scope_id = 101;

    let received = Arc::new(Mutex::new(Vec::new()));
    let completed = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));

    let r_clone = received.clone();
    let c_clone = completed.clone();
    let f_clone = failed.clone();

    let observer = Observer::new(
        move |item: i64| {
            r_clone.lock().unwrap().push(item);
            Ok(())
        },
        move || {
            c_clone.store(true, Ordering::SeqCst);
        },
        move |_: RxError<String>| {
            f_clone.store(true, Ordering::SeqCst);
        },
    );

    let stream: Observable<i64, String> = Observable::from_iter(vec![10, 20, 30]);
    let sub = stream
        .subscribe(
            observer,
            scope_id,
            BufferOptions::default_bounded(),
            scheduler.clone(),
        )
        .unwrap();

    scheduler.run_until_quiescent(100);

    let completion = sub.join(Some(Duration::from_millis(100))).unwrap();
    assert_eq!(completion.exit, ExitStatus::Completed);
    assert_eq!(completion.items_delivered, 3);
    assert_eq!(*received.lock().unwrap(), vec![10, 20, 30]);
    assert!(completed.load(Ordering::SeqCst));
    assert!(!failed.load(Ordering::SeqCst));
}

#[test]
fn test_rx_scope_cancel_zero_lingering_callbacks() {
    let scheduler = Arc::new(RxScheduler::new_virtual());
    let scope_id = 102;

    let received = Arc::new(Mutex::new(Vec::new()));
    let completed = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(AtomicBool::new(false));

    let r_clone = received.clone();
    let c_clone = completed.clone();
    let f_clone = failed.clone();

    let observer = Observer::new(
        move |item: i64| {
            r_clone.lock().unwrap().push(item);
            Ok(())
        },
        move || {
            c_clone.store(true, Ordering::SeqCst);
        },
        move |_: RxError<String>| {
            f_clone.store(true, Ordering::SeqCst);
        },
    );

    // Continuous producer
    let stream: Observable<i64, String> = Observable::from_producer(move |control| {
        for i in 0..100 {
            if !control.is_active() {
                break;
            }
            if let Err(_) = control.deliver_next(i) {
                break;
            }
        }
        control.complete();
        Ok(())
    });

    let sub = stream
        .subscribe(
            observer,
            scope_id,
            BufferOptions::default_bounded(),
            scheduler.clone(),
        )
        .unwrap();

    // Cancel before running scheduler
    sub.cancel();

    scheduler.run_until_quiescent(100);

    let completion = sub.join(Some(Duration::from_millis(100))).unwrap();
    assert_eq!(completion.exit, ExitStatus::Cancelled);
    // Cancellation guarantees zero terminal observer callbacks (RFC §16.3)
    assert!(!completed.load(Ordering::SeqCst));
    assert!(!failed.load(Ordering::SeqCst));
    assert_eq!(received.lock().unwrap().len(), 0);
}

#[test]
fn test_rx_self_unsubscribe_and_deadlock_prevention() {
    let scheduler = Arc::new(RxScheduler::new_virtual());
    let scope_id = 103;

    let received = Arc::new(Mutex::new(Vec::new()));
    let self_join_error_observed = Arc::new(AtomicBool::new(false));
    let sub_holder: Arc<Mutex<Option<Subscription<i64, String>>>> = Arc::new(Mutex::new(None));

    let r_clone = received.clone();
    let sub_clone = sub_holder.clone();
    let sj_clone = self_join_error_observed.clone();

    let observer = Observer::new(
        move |item: i64| {
            r_clone.lock().unwrap().push(item);
            if item == 2 {
                if let Some(ref s) = *sub_clone.lock().unwrap() {
                    // Attempt self-join from callback: MUST fail with SelfJoin defect
                    if let Err(RxError::SelfJoin) = s.join(Some(Duration::from_millis(10))) {
                        sj_clone.store(true, Ordering::SeqCst);
                    }
                    // Self-cancel from callback
                    s.cancel();
                }
            }
            Ok(())
        },
        || {},
        |_: RxError<String>| {},
    );

    let stream: Observable<i64, String> = Observable::from_iter(vec![1, 2, 3, 4, 5]);
    let sub = stream
        .subscribe(
            observer,
            scope_id,
            BufferOptions::default_bounded(),
            scheduler.clone(),
        )
        .unwrap();

    *sub_holder.lock().unwrap() = Some(sub.clone());

    scheduler.run_until_quiescent(100);

    // Self-join was intercepted and prevented
    assert!(self_join_error_observed.load(Ordering::SeqCst));

    // Callback 3, 4, 5 were never executed
    assert_eq!(*received.lock().unwrap(), vec![1, 2]);

    // External join succeeds
    let completion = sub.join(Some(Duration::from_millis(100))).unwrap();
    assert_eq!(completion.exit, ExitStatus::Cancelled);
}

#[test]
fn test_rx_interval_ticks_and_cancellation() {
    let scheduler = Arc::new(RxScheduler::new_virtual());
    let scope_id = 104;

    let ticks = Arc::new(Mutex::new(Vec::new()));
    let t_clone = ticks.clone();

    let observer: Observer<i64, String> = Observer::from_next(move |val: i64| {
        t_clone.lock().unwrap().push(val);
        Ok(())
    });

    let stream = interval::<String>(Duration::from_millis(100), scheduler.clone());
    let sub = stream
        .subscribe(
            observer,
            scope_id,
            BufferOptions::default_bounded(),
            scheduler.clone(),
        )
        .unwrap();

    // Initial state: t=0, execute initial setup task
    scheduler.drain_ready();
    assert_eq!(ticks.lock().unwrap().len(), 0);

    // Advance clock by 100ms increments (ticks at 100ms, 200ms, 300ms)
    scheduler.advance_and_drain(Duration::from_millis(100)); // tick 0
    scheduler.advance_and_drain(Duration::from_millis(100)); // tick 1
    scheduler.advance_and_drain(Duration::from_millis(100)); // tick 2
    scheduler.advance_and_drain(Duration::from_millis(50));  // t=350ms, next tick at 400ms

    assert_eq!(*ticks.lock().unwrap(), vec![0, 1, 2]);

    // Cancel interval stream
    sub.cancel();

    // Advance by another 500ms -> no new ticks delivered
    scheduler.advance_and_drain(Duration::from_millis(500));
    assert_eq!(*ticks.lock().unwrap(), vec![0, 1, 2]);

    let completion = sub.join(Some(Duration::from_millis(100))).unwrap();
    assert_eq!(completion.exit, ExitStatus::Cancelled);
}

#[test]
fn test_rx_terminal_error_propagation_once() {
    let scheduler = Arc::new(RxScheduler::new_virtual());
    let scope_id = 105;

    let fail_count = Arc::new(AtomicUsize::new(0));
    let fc = fail_count.clone();

    let observer = Observer::new(
        |_: i64| Ok(()),
        || panic!("Should not complete on error"),
        move |err: RxError<String>| {
            fc.fetch_add(1, Ordering::SeqCst);
            assert_eq!(err, RxError::Source("simulated error".to_string()));
        },
    );

    let stream = Observable::from_producer(|control| {
        control.deliver_next(1)?;
        control.fail(RxError::Source("simulated error".to_string()));
        // Duplicate fail call should be inert
        control.fail(RxError::Source("second error".to_string()));
        control.complete();
        Ok(())
    });

    let sub = stream
        .subscribe(
            observer,
            scope_id,
            BufferOptions::default_bounded(),
            scheduler.clone(),
        )
        .unwrap();

    scheduler.run_until_quiescent(100);

    let completion = sub.join(Some(Duration::from_millis(100))).unwrap();
    assert_eq!(
        completion.exit,
        ExitStatus::Failed(RxError::Source("simulated error".to_string()))
    );
    assert_eq!(fail_count.load(Ordering::SeqCst), 1);
}
