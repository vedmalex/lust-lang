use std::cell::RefCell;
use std::rc::Rc;
use lust::{
    effect_abi::{
        Cause, Exit, FiberContext, FiberExecutionState, InterruptReason, ScheduleKind, Scope,
        ScopeAdmission, ScopeTeardown, effect_acquire_release, effect_repeat, effect_retry,
        effect_timeout,
    },
    vm::{TaskId, TaskState, VM},
};

#[test]
fn test_gate_neg_02_scope_waits_for_child_task_before_releasing_host_resource() {
    let mut vm = VM::new();
    let handle = vm.create_native_future_task();
    let mut scope = Scope::new(100, None);

    // Register child task into parent scope
    vm.register_scope_task(&mut scope, handle)
        .expect("child task should be registered into scope");

    // Track execution order of resource finalizers
    let events = Rc::new(RefCell::new(Vec::new()));
    for id in [1, 2] {
        let ev = events.clone();
        scope
            .register_finalizer(move || {
                ev.borrow_mut().push(format!("release_host_resource_{}", id));
            })
            .expect("register finalizer must succeed on open scope");
    }

    // Emulate child task entering an interrupt-masked section (holding host resource)
    let _task_id = TaskId(handle.id());
    vm.get_task_instance_mut(handle)
        .unwrap()
        .fiber_context
        .enter_mask();

    // INVARIANT GATE-NEG-02:
    // When parent scope begins teardown, it CANNOT finalize host resources while child is running or masked
    let closed = vm.close_scope(&mut scope);
    assert!(!closed, "Scope cannot close while child task is executing");
    assert_eq!(
        scope.teardown,
        ScopeTeardown::Joining,
        "Scope teardown stage must be Joining"
    );
    assert_eq!(
        scope.admission,
        ScopeAdmission::Closed,
        "Scope must refuse new child admissions"
    );
    assert!(
        scope.register_child(999).is_err(),
        "Scope cannot register new children once closing"
    );

    // No host resource finalizer has executed yet!
    assert!(
        events.borrow().is_empty(),
        "Host resources must NOT be released while child task is active"
    );

    // Child task received pending cancellation
    let task = vm.get_task_instance_mut(handle).unwrap();
    assert_eq!(
        task.fiber_context.pending_interrupt,
        Some(InterruptReason::Cancelled)
    );

    // Now child exits masked critical section and delivers deferred interrupt
    let pending = task.fiber_context.exit_mask().unwrap();
    assert_eq!(pending, InterruptReason::Cancelled);
    task.request_interrupt(pending);
    assert_eq!(task.state, TaskState::Stopped);

    // Second close_scope invocation: all children joined, proceeds to LIFO finalization
    let closed_now = vm.close_scope(&mut scope);
    assert!(closed_now, "Scope must close successfully after all children stopped");
    assert_eq!(scope.teardown, ScopeTeardown::Done);

    // INVARIANT: Finalizers ran strictly in reverse (LIFO) order
    assert_eq!(
        &*events.borrow(),
        &["release_host_resource_2", "release_host_resource_1"]
    );

    // Successive invocations are idempotent and safe
    assert!(vm.close_scope(&mut scope));
    assert_eq!(events.borrow().len(), 2);
}

#[test]
fn test_effect_retry_recurs_attempts_and_fail_closed_contract() {
    // recurs(0): exactly 1 attempt total
    let attempts_0 = Rc::new(RefCell::new(0));
    let attempts_0_c = attempts_0.clone();
    let res_0: Exit<i32, &'static str> = effect_retry(
        || {
            *attempts_0_c.borrow_mut() += 1;
            Exit::Failure(Cause::Fail("transient"))
        },
        &ScheduleKind::recurs(0),
    );
    assert_eq!(*attempts_0.borrow(), 1);
    assert_eq!(res_0, Exit::Failure(Cause::Fail("transient")));

    // recurs(2): 1 initial + 2 retries = 3 attempts total
    let attempts_2 = Rc::new(RefCell::new(0));
    let attempts_2_c = attempts_2.clone();
    let res_2: Exit<i32, &'static str> = effect_retry(
        || {
            *attempts_2_c.borrow_mut() += 1;
            Exit::Failure(Cause::Fail("network"))
        },
        &ScheduleKind::recurs(2),
    );
    assert_eq!(*attempts_2.borrow(), 3);
    assert_eq!(res_2, Exit::Failure(Cause::Fail("network")));

    // Fail-closed invariant: Cause::Interrupted (OutOfGas) CANNOT be retried
    let attempts_oog = Rc::new(RefCell::new(0));
    let attempts_oog_c = attempts_oog.clone();
    let res_oog: Exit<i32, &'static str> = effect_retry(
        || {
            *attempts_oog_c.borrow_mut() += 1;
            Exit::Failure(Cause::Interrupted(InterruptReason::OutOfGas))
        },
        &ScheduleKind::recurs(10),
    );
    assert_eq!(
        *attempts_oog.borrow(),
        1,
        "Sticky interrupt OutOfGas must not be retried"
    );
    assert_eq!(
        res_oog,
        Exit::Failure(Cause::Interrupted(InterruptReason::OutOfGas))
    );
}

#[test]
fn test_effect_repeat_successful_iterations() {
    let count = Rc::new(RefCell::new(0));
    let count_c = count.clone();

    // recurs(3) = 1 initial + 3 repeats = 4 executions
    let res: Exit<i32, ()> = effect_repeat(
        || {
            *count_c.borrow_mut() += 1;
            Exit::Success(*count_c.borrow())
        },
        &ScheduleKind::recurs(3),
    );

    assert_eq!(*count.borrow(), 4);
    assert_eq!(res, Exit::Success(4));
}

#[test]
fn test_effect_timeout_interruption() {
    let success: Exit<i32, &'static str> = effect_timeout(|| Exit::Success(123), false);
    assert_eq!(success, Exit::Success(123));

    let timed_out: Exit<i32, &'static str> = effect_timeout(|| Exit::Success(123), true);
    assert_eq!(
        timed_out,
        Exit::Failure(Cause::Interrupted(InterruptReason::Timeout))
    );
}

#[test]
fn test_effect_acquire_release_interrupt_masking() {
    let mut fiber = FiberContext::new(200, 1);
    let mut scope = Scope::new(1, None);
    let released = Rc::new(RefCell::new(false));
    let released_c = released.clone();

    let res: Exit<&'static str, &'static str> = effect_acquire_release(
        &mut fiber,
        &mut scope,
        |fib| {
            // Emulate an interrupt (e.g. Timeout) arriving while inside acquire
            let delivered = fib.request_interrupt(InterruptReason::Timeout);
            assert!(!delivered, "Interrupt must be masked during acquire");
            assert_eq!(fib.pending_interrupt, Some(InterruptReason::Timeout));
            Exit::Success("resource_acquired")
        },
        move || {
            *released_c.borrow_mut() = true;
        },
    );

    // Finalizer was registered
    assert_eq!(scope.finalizers.len(), 1);
    // Upon unmasking, deferred interrupt was delivered
    assert_eq!(
        res,
        Exit::Failure(Cause::Interrupted(InterruptReason::Timeout))
    );
    assert_eq!(fiber.state, FiberExecutionState::Interrupted);

    // Releasing scope executes the finalizer
    scope.finalize_resources();
    assert!(*released.borrow(), "Finalizer must be executed on teardown");
}
