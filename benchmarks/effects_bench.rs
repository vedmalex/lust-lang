use lust::effect_abi::{
    acquire_release, effect_timeout, repeat, retry, Cause, Exit, FiberContext,
    InterruptReason, LustEffectDescriptor, LUST_EFFECT_ABI_V1, ScheduleKind,
    ScheduleState, ScheduleStep, Scope, ScopeAdmission, ScopeTeardown,
    StackContextProjection,
};
use lust::test_framework::{
    assert_eq, assert_fails, assert_success, MockService, TestClockState,
};
use std::hint::black_box;
use std::time::Instant;

unsafe extern "C" fn noop_effect_invoke(
    _userdata: *mut core::ffi::c_void,
    _rt: *mut core::ffi::c_void,
    _env: *const core::ffi::c_void,
    _out_exit: *mut core::ffi::c_void,
) -> lust::effect_abi::LustStatus {
    lust::effect_abi::LUST_STATUS_SUCCESS
}

unsafe extern "C" fn noop_effect_release(
    _userdata: *mut core::ffi::c_void,
    _rt: *mut core::ffi::c_void,
) {}

fn main() {
    println!("=================================================================");
    println!("          LUST LANG EFFECT SYSTEM PERFORMANCE BENCHMARK          ");
    println!("=================================================================\n");

    // =========================================================================
    // SECTION 1: Direct Native Baseline & C-ABI
    // =========================================================================
    println!("--- [Section 1: Direct Native & C-ABI Descriptors] ---");
    const ITERS_BASE: u64 = 10_000_000;

    let start = Instant::now();
    let mut sum: u64 = 0;
    for i in 0..ITERS_BASE {
        sum = sum.wrapping_add(black_box(i));
    }
    let elapsed = start.elapsed();
    println!(
        "  1.1 Direct Native Baseline:       {:8.2} ns/op  ({:?} total, sum={})",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed,
        sum
    );

    let descriptor = LustEffectDescriptor {
        abi_version: LUST_EFFECT_ABI_V1,
        userdata: std::ptr::null_mut(),
        invoke_fn: Some(noop_effect_invoke),
        release_fn: Some(noop_effect_release),
    };
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let code = if let Some(inv) = descriptor.invoke_fn {
            unsafe {
                inv(
                    descriptor.userdata,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            }
        } else {
            0
        };
        black_box(code);
    }
    let elapsed = start.elapsed();
    println!(
        "  1.2 LustEffectDescriptor (C-ABI): {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // =========================================================================
    // SECTION 2: StackContextProjection Zero-Heap Lookups
    // =========================================================================
    println!("\n--- [Section 2: StackContextProjection (Zero-Heap O(1) Lookup)] ---");
    let dummy_val: u64 = 42;
    let ptr = &dummy_val as *const u64 as *const core::ffi::c_void;

    // N = 1
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let projection = StackContextProjection::new([ptr], [101u32]);
        let found = projection.get(101);
        black_box(found);
    }
    let elapsed = start.elapsed();
    println!(
        "  2.1 StackContextProjection<1>:    {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // N = 2
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let projection = StackContextProjection::new([ptr, ptr], [101u32, 202u32]);
        let found = projection.get(202);
        black_box(found);
    }
    let elapsed = start.elapsed();
    println!(
        "  2.2 StackContextProjection<2>:    {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // N = 4
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let projection =
            StackContextProjection::new([ptr, ptr, ptr, ptr], [101u32, 202u32, 303u32, 404u32]);
        let found = projection.get(404);
        black_box(found);
    }
    let elapsed = start.elapsed();
    println!(
        "  2.3 StackContextProjection<4>:    {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // =========================================================================
    // SECTION 3: Schedule Policies & Algebraic Combinators
    // =========================================================================
    println!("\n--- [Section 3: Schedule Policies & Algebraic Combinators] ---");

    // 3.1 Schedule step raw
    let sched_recurs = ScheduleKind::recurs(10);
    let mut state = ScheduleState::initial();
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        state = match sched_recurs.step(&state) {
            ScheduleStep::Continue { next_state, .. } => next_state,
            ScheduleStep::Done => ScheduleState::initial(),
        };
        black_box(&state);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.1 ScheduleKind::step (recurs):   {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    let sched_exp = ScheduleKind::exponential(10, 1.5, 1000);
    let mut state = ScheduleState::initial();
    const EXP_ITERS: u64 = 5_000_000;
    let start = Instant::now();
    for _ in 0..EXP_ITERS {
        state = match sched_exp.step(&state) {
            ScheduleStep::Continue { next_state, .. } => next_state,
            ScheduleStep::Done => ScheduleState::initial(),
        };
        black_box(&state);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.2 ScheduleKind::step (exponential): {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / EXP_ITERS as f64,
        elapsed
    );

    // 3.3 retry (immediate recurs backoff, 3 retries then success)
    const COMB_ITERS: u64 = 1_000_000;
    let sched_retry_imm = ScheduleKind::recurs(3);
    let start = Instant::now();
    for _ in 0..COMB_ITERS {
        let mut count: u32 = 0;
        let res = retry(
            || {
                count += 1;
                if count <= 2 {
                    Exit::<u64, &'static str>::Failure(Cause::Fail("transient"))
                } else {
                    Exit::<u64, &'static str>::Success(42)
                }
            },
            &sched_retry_imm,
        );
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.3 retry (immediate recurs, 2 fails -> ok): {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / COMB_ITERS as f64,
        elapsed
    );

    // 3.4 retry (spaced backoff step simulation, 3 retries then success)
    let sched_retry_spaced = ScheduleKind::spaced(50);
    let start = Instant::now();
    for _ in 0..COMB_ITERS {
        let mut count: u32 = 0;
        let res = retry(
            || {
                count += 1;
                if count <= 2 {
                    Exit::<u64, &'static str>::Failure(Cause::Fail("transient"))
                } else {
                    Exit::<u64, &'static str>::Success(100)
                }
            },
            &sched_retry_spaced,
        );
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.4 retry (spaced backoff, 2 fails -> ok):    {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / COMB_ITERS as f64,
        elapsed
    );

    // 3.5 retry (exponential backoff step simulation, 3 retries then success)
    let sched_retry_exp = ScheduleKind::exponential(10, 2.0, 500);
    let start = Instant::now();
    for _ in 0..COMB_ITERS {
        let mut count: u32 = 0;
        let res = retry(
            || {
                count += 1;
                if count <= 2 {
                    Exit::<u64, &'static str>::Failure(Cause::Fail("transient"))
                } else {
                    Exit::<u64, &'static str>::Success(200)
                }
            },
            &sched_retry_exp,
        );
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.5 retry (exponential backoff, 2 fails -> ok):{:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / COMB_ITERS as f64,
        elapsed
    );

    // 3.6 repeat (recurs(4) = 5 successful executions)
    let sched_repeat = ScheduleKind::recurs(4);
    let start = Instant::now();
    for _ in 0..COMB_ITERS {
        let mut count: u32 = 0;
        let res = repeat(
            || {
                count += 1;
                Exit::<u32, ()>::Success(count)
            },
            &sched_repeat,
        );
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.6 repeat (recurs 5 executions):            {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / COMB_ITERS as f64,
        elapsed
    );

    // 3.7 timeout (effect_timeout fast-path success)
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let res = effect_timeout(|| Exit::<u64, ()>::Success(42), false);
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.7 effect_timeout (success deadline met):    {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // 3.8 timeout (effect_timeout cutoff interrupt)
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let res = effect_timeout(|| Exit::<u64, ()>::Success(42), true);
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.8 effect_timeout (cutoff -> Timeout interrupt):{:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // 3.9 acquire_release with interrupt masking & LIFO finalizer
    let mut fiber = FiberContext::new(1, 1);
    let mut scope = Scope::new(1, None);
    let start = Instant::now();
    for _ in 0..COMB_ITERS {
        let res = acquire_release(
            &mut fiber,
            &mut scope,
            |_| Exit::<u64, ()>::Success(42),
            || {},
        );
        black_box(res);
        scope.finalizers.clear();
    }
    let elapsed = start.elapsed();
    println!(
        "  3.9 acquire_release + Masking (uninterrupted):{:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / COMB_ITERS as f64,
        elapsed
    );

    // 3.10 acquire_release with interrupt delivered during masked acquire
    let start = Instant::now();
    for _ in 0..COMB_ITERS {
        let res = acquire_release(
            &mut fiber,
            &mut scope,
            |fib| {
                fib.request_interrupt(InterruptReason::Cancelled);
                Exit::<u64, ()>::Success(42)
            },
            || {},
        );
        black_box(res);
        scope.finalizers.clear();
    }
    let elapsed = start.elapsed();
    println!(
        "  3.10 acquire_release + Deferred Interrupt:   {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / COMB_ITERS as f64,
        elapsed
    );

    // =========================================================================
    // SECTION 4: Scope Lifecycle & Child Fiber Cancellation (Gate-NEG-02)
    // =========================================================================
    println!("\n--- [Section 4: Scope Lifecycle & Child Cancellation (Gate-NEG-02)] ---");
    const SCOPE_ITERS: u64 = 500_000;

    // 4.1 Standard Scope Lifecycle: create, register 2 children, 2 finalizers, teardown, LIFO finalize
    let start = Instant::now();
    for i in 0..SCOPE_ITERS {
        let mut scope = Scope::new(i, None);
        let _ = scope.register_child(i * 10 + 1);
        let _ = scope.register_child(i * 10 + 2);
        let _ = scope.register_finalizer(|| {});
        let _ = scope.register_finalizer(|| {});

        let children = scope.begin_teardown();
        for _cid in children {
            // Simulated cancellation signal delivery
            black_box(_cid);
        }
        scope.mark_joining();
        scope.finalize_resources();
        black_box(&scope);
    }
    let elapsed = start.elapsed();
    println!(
        "  4.1 Scope Lifecycle (2 children + 2 LIFO finalizers): {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / SCOPE_ITERS as f64,
        elapsed
    );

    // 4.2 Gate-NEG-02 Scope Teardown with Interrupt-Masked Child Fiber
    // Invariant: Parent scope begins teardown -> signals Cancelled -> child is masked ->
    // deferred interrupt stored -> child exits mask -> delivers interrupt -> scope marks joining & finalizes.
    let start = Instant::now();
    for i in 0..SCOPE_ITERS {
        let mut scope = Scope::new(i, None);
        let mut child_fiber = FiberContext::new(i * 10 + 1, i);
        let _ = scope.register_child(child_fiber.id);
        let _ = scope.register_finalizer(|| {});

        // Child enters masked critical section (e.g. holding host resource)
        child_fiber.enter_mask();

        // Scope begins teardown: admission closed, transitions to Cancelling
        let children = scope.begin_teardown();
        for _cid in children {
            let delivered = child_fiber.request_interrupt(InterruptReason::Cancelled);
            debug_assert!(!delivered, "Interrupt must be deferred while masked");
        }
        debug_assert_eq!(scope.admission, ScopeAdmission::Closed);
        debug_assert_eq!(scope.teardown, ScopeTeardown::Cancelling);

        // Scope cannot finalize resources while child is still masked/running!
        scope.mark_joining();
        debug_assert_eq!(scope.teardown, ScopeTeardown::Joining);

        // Child exits masked section: pending interrupt unmasks
        let pending = child_fiber.exit_mask();
        debug_assert_eq!(pending, Some(InterruptReason::Cancelled));
        child_fiber.request_interrupt(pending.unwrap());

        // Now child is joined -> scope finalizes resources in LIFO order
        scope.finalize_resources();
        debug_assert_eq!(scope.teardown, ScopeTeardown::Done);
        black_box((&scope, &child_fiber));
    }
    let elapsed = start.elapsed();
    println!(
        "  4.2 Gate-NEG-02 Masked Teardown + Deferred Cancel:    {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / SCOPE_ITERS as f64,
        elapsed
    );

    // =========================================================================
    // SECTION 5: Test Framework Micro-benchmarks
    // =========================================================================
    println!("\n--- [Section 5: Testing Framework (@std/test) Assertions] ---");

    // 5.1 assert_eq passing
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let r = assert_eq(black_box(42), black_box(42), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  5.1 assert_eq (passing integers):            {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // 5.2 assert_eq failing with diff formatting
    const FAIL_ITERS: u64 = 100_000;
    let start = Instant::now();
    for _ in 0..FAIL_ITERS {
        let r = assert_eq(black_box(42), black_box(99), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  5.2 assert_eq (failing + format_diff):       {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / FAIL_ITERS as f64,
        elapsed
    );

    // 5.3 assert_success
    let ok_exit = Exit::<u64, ()>::Success(42);
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let r = assert_success(black_box(&ok_exit), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  5.3 assert_success (Exit::Success):          {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // 5.4 assert_fails
    let fail_exit = Exit::<(), &'static str>::Failure(Cause::Fail("ECONNRESET"));
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let r = assert_fails(black_box(&fail_exit), Some(&"ECONNRESET"), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  5.4 assert_fails (Exit::Failure Cause::Fail):{:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    // =========================================================================
    // SECTION 6: Virtual Time Engine (TestClock) vs Physical Time
    // =========================================================================
    println!("\n--- [Section 6: Virtual Time Engine (TestClock) vs Physical Time] ---");
    const CLOCK_ITERS: u64 = 500_000;

    let start = Instant::now();
    for _ in 0..CLOCK_ITERS {
        let mut clock = TestClockState::new();
        // Schedule 5 timers at varying delays
        clock.schedule(1, 10);
        clock.schedule(2, 50);
        clock.schedule(3, 100);
        clock.schedule(4, 500);
        clock.schedule(5, 1000);

        // Fast forward 1000ms: awaken all 5 timers in a single warp adjustment
        let awakened = clock.adjust(1000);
        black_box(awakened);
    }
    let elapsed_virtual = start.elapsed();
    let ns_per_warp = elapsed_virtual.as_nanos() as f64 / CLOCK_ITERS as f64;
    println!(
        "  6.1 TestClock.adjust (warp 5 timers / 1000ms):{:8.2} ns/op  ({:?} total)",
        ns_per_warp, elapsed_virtual
    );

    // Compare with physical time resolution
    let phys_start = Instant::now();
    for _ in 0..ITERS_BASE {
        let t = Instant::now();
        black_box(t);
    }
    let phys_elapsed = phys_start.elapsed();
    println!(
        "  6.2 Instant::now (physical timestamp query): {:8.2} ns/op  ({:?} total)",
        phys_elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        phys_elapsed
    );
    let speedup = 1_000_000_000.0 / ns_per_warp;
    println!(
        "  6.3 Virtual Time Acceleration:               {:.2e}x faster than real-time execution",
        speedup
    );

    // =========================================================================
    // SECTION 7: Mocking Engine & Zero-Heap StackContextProjection
    // =========================================================================
    println!("\n--- [Section 7: Mocking Engine & Context Projection] ---");
    const MOCK_ITERS: u64 = 50_000;

    let mock = MockService::new(101);
    let start = Instant::now();
    for i in 0..MOCK_ITERS {
        mock.record_call("compute", &["arg_a", "arg_b"], i);
    }
    let elapsed = start.elapsed();
    println!(
        "  7.1 MockService.record_call:                  {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / MOCK_ITERS as f64,
        elapsed
    );

    let small_mock = MockService::new(202);
    for i in 0..20 {
        small_mock.record_call("compute", &["arg_a", "arg_b"], i);
    }
    let start = Instant::now();
    for _ in 0..MOCK_ITERS {
        let cnt = small_mock.call_count("compute");
        black_box(cnt);
    }
    let elapsed = start.elapsed();
    println!(
        "  7.2 MockService.call_count (20 entries):      {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / MOCK_ITERS as f64,
        elapsed
    );

    let mock1 = MockService::new(1001);
    let mock2 = MockService::new(1002);
    let start = Instant::now();
    for _ in 0..ITERS_BASE {
        let proj = MockService::to_projection_pair(&mock1, &mock2);
        let ptr = proj.get(1002);
        black_box(ptr);
    }
    let elapsed = start.elapsed();
    println!(
        "  7.2 MockService -> StackContextProjection<2>: {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        elapsed.as_nanos() as f64 / ITERS_BASE as f64,
        elapsed
    );

    println!("\n=================================================================");
    println!("                   BENCHMARK COMPLETED                           ");
    println!("=================================================================");
}
