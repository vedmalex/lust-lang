//! Example 6: Structured Concurrency, Fiber Supervision & Gate-NEG-02.
//! Demonstrates fiber hierarchies in `Scope`, cascading cancellation on failure,
//! and guaranteed LIFO finalization of host resources (RFC §3.4 / Gate-NEG-02).

use lust::effect_abi::{
    FiberContext, FiberExecutionState, InterruptReason, Scope,
    ScopeAdmission, ScopeTeardown,
};
use lust::test_framework::{assert, assert_eq};
use std::cell::RefCell;
use std::rc::Rc;

/// Mock host resource with state tracking.
struct HostResource {
    #[allow(dead_code)]
    name: &'static str,
    active: Rc<RefCell<bool>>,
}

impl HostResource {
    fn acquire(
        name: &'static str,
        event_log: Rc<RefCell<Vec<String>>>,
    ) -> (Self, Box<dyn FnOnce()>) {
        println!("  [HostResource::acquire] Allocating: {}", name);
        event_log.borrow_mut().push(format!("acquire:{}", name));
        let active = Rc::new(RefCell::new(true));

        let active_clone = active.clone();
        let log_clone = event_log.clone();
        let release = Box::new(move || {
            *active_clone.borrow_mut() = false;
            println!("  [HostResource::release] LIFO teardown: {}", name);
            log_clone.borrow_mut().push(format!("release:{}", name));
        });

        (Self { name, active }, release)
    }
}

fn main() {
    println!("--- [Example 6: Structured Concurrency & Supervision (Gate-NEG-02)] ---");

    let event_log = Rc::new(RefCell::new(Vec::new()));

    // -----------------------------------------------------------------------
    // Part 1: Scope Initialization and Resource Acquisition
    // -----------------------------------------------------------------------
    println!("\n1. Initializing Root Scope #100 and acquiring scoped resources:");
    let mut scope = Scope::new(100, None);
    assert_eq(scope.is_open(), true, None).unwrap();
    assert_eq(scope.teardown, ScopeTeardown::Idle, None).unwrap();

    // Acquire resources in sequential order: 1 -> 2 -> 3
    let (res1, rel1) = HostResource::acquire("PostgreSQL Connection Pool", event_log.clone());
    scope.register_finalizer(rel1).unwrap();

    let (res2, rel2) = HostResource::acquire("TCP Socket Listener", event_log.clone());
    scope.register_finalizer(rel2).unwrap();

    let (res3, rel3) = HostResource::acquire("Worker Scratch Arena Buffer", event_log.clone());
    scope.register_finalizer(rel3).unwrap();

    assert_eq(scope.finalizers.len(), 3, None).unwrap();
    println!("  Scope #100 active with {} registered finalizers.", scope.finalizers.len());

    // -----------------------------------------------------------------------
    // Part 2: Spawning Child Fibers in Structured Scope
    // -----------------------------------------------------------------------
    println!("\n2. Spawning child fiber tree under Scope #100:");
    let mut fiber_a = FiberContext::new(101, 100); // Fiber 101: Telemetry Poller
    let mut fiber_b = FiberContext::new(102, 100); // Fiber 102: Data Pipeline Processor
    let mut fiber_c = FiberContext::new(103, 100); // Fiber 103: Cache Sync Worker

    scope.register_child(fiber_a.id).unwrap();
    scope.register_child(fiber_b.id).unwrap();
    scope.register_child(fiber_c.id).unwrap();
    println!("  Registered children in scope: {:?}", scope.children);
    assert_eq(scope.children.clone(), vec![101, 102, 103], None).unwrap();

    // Fiber B enters an interrupt-masked critical section (holding critical state)
    println!("  Fiber #102 enters interrupt-masked section (depth = 1)...");
    fiber_b.enter_mask();
    assert_eq(fiber_b.is_masked(), true, None).unwrap();

    // -----------------------------------------------------------------------
    // Part 3: Failure in Worker C & Cascading Scope Supervision
    // -----------------------------------------------------------------------
    println!("\n3. Worker #103 experiences fatal pipeline failure; Supervisor begins teardown:");
    fiber_c.state = FiberExecutionState::Failed;

    // Supervisor begins scope teardown:
    let children_to_cancel = scope.begin_teardown();
    println!("  Children targeted for cascading cancellation: {:?}", children_to_cancel);

    // INVARIANT 1: Admission is strictly closed
    assert_eq(scope.admission, ScopeAdmission::Closed, None).unwrap();
    assert_eq(scope.teardown, ScopeTeardown::Cancelling, None).unwrap();

    let spawn_rejected = scope.register_child(104);
    println!("  Attempting to spawn Worker #104 in closing scope: {:?}", spawn_rejected);
    assert(spawn_rejected.is_err(), "Must reject new children once closing", None).unwrap();

    let acq_rejected = scope.register_finalizer(|| {});
    println!("  Attempting to acquire resource in closing scope: {:?}", acq_rejected);
    assert(acq_rejected.is_err(), "Must reject new finalizers once closing", None).unwrap();

    // INVARIANT 2: Signal cancellation to all active children
    println!("\n4. Broadcasting cancellation interrupts to child fibers:");
    for child_id in children_to_cancel {
        match child_id {
            101 => {
                let del = fiber_a.request_interrupt(InterruptReason::Cancelled);
                fiber_a.state = FiberExecutionState::Interrupted;
                println!("    Delivered interrupt to Fiber #101 immediately: {}", del);
            }
            102 => {
                let del = fiber_b.request_interrupt(InterruptReason::Cancelled);
                println!(
                    "    Delivered interrupt to Fiber #102 (masked): {} (deferred: {:?})",
                    del, fiber_b.pending_interrupt
                );
                assert_eq(del, false, None).unwrap(); // Deferred because fiber_b is masked!
            }
            103 => {
                let _ = fiber_c.request_interrupt(InterruptReason::Cancelled);
            }
            _ => unreachable!(),
        }
    }

    // -----------------------------------------------------------------------
    // Part 4: Gate-NEG-02 Verification (Scope Teardown Stage: Joining)
    // -----------------------------------------------------------------------
    println!("\n5. Verifying Gate-NEG-02 Invariant during Joining stage:");
    scope.mark_joining();
    assert_eq(scope.teardown, ScopeTeardown::Joining, None).unwrap();

    // INVARIANT Gate-NEG-02:
    // Host resources MUST NOT be released while any child fiber is active or masked!
    let current_releases: Vec<String> = event_log
        .borrow()
        .iter()
        .filter(|e| e.starts_with("release:"))
        .cloned()
        .collect();
    println!(
        "  Active releases before child fibers join: {:?} (MUST BE EMPTY!)",
        current_releases
    );
    assert(
        current_releases.is_empty(),
        "Gate-NEG-02 VIOLATION: Host resources finalized before children joined!",
        None,
    ).unwrap();

    // Now Worker B completes its critical work, unmasks, and receives deferred interrupt
    println!("\n6. Worker #102 exits masked section and delivers deferred cancellation:");
    let deferred = fiber_b.exit_mask().expect("Pending interrupt must be delivered on unmask");
    println!("  Fiber #102 delivered interrupt: {:?}", deferred);
    assert_eq(deferred, InterruptReason::Cancelled, None).unwrap();
    fiber_b.state = FiberExecutionState::Interrupted;
    assert_eq(fiber_b.state, FiberExecutionState::Interrupted, None).unwrap();

    // All children have now stopped and joined!
    println!("  All child fibers [101, 102, 103] confirmed joined.");

    // -----------------------------------------------------------------------
    // Part 5: Guaranteed LIFO Finalization of Host Resources
    // -----------------------------------------------------------------------
    println!("\n7. Executing finalizers in reverse (LIFO) order:");
    scope.finalize_resources();
    assert_eq(scope.teardown, ScopeTeardown::Done, None).unwrap();

    let all_events = event_log.borrow().clone();
    println!("  Complete execution event log:");
    for (i, ev) in all_events.iter().enumerate() {
        println!("    {}. {}", i + 1, ev);
    }

    // Verify LIFO order of teardown: 3 -> 2 -> 1
    let releases: Vec<String> = all_events
        .into_iter()
        .filter(|e| e.starts_with("release:"))
        .collect();

    assert_eq(
        releases,
        vec![
            "release:Worker Scratch Arena Buffer".to_string(),
            "release:TCP Socket Listener".to_string(),
            "release:PostgreSQL Connection Pool".to_string(),
        ],
        None
    ).unwrap();

    assert_eq(*res1.active.borrow(), false, None).unwrap();
    assert_eq(*res2.active.borrow(), false, None).unwrap();
    assert_eq(*res3.active.borrow(), false, None).unwrap();

    println!("\n[PASS] Structured Concurrency, Cascading Supervision, and Gate-NEG-02 verified!");
}
