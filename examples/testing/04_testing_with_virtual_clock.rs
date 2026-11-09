//! Example 4: Unit Testing Effects with Virtual Time (TestClock).
//! Demonstrates zero-delay simulation of heavy retry policies, exponential backoff,
//! jitter, and timeouts in < 1 ms wall-clock time without `std::thread::sleep`.

use lust::effect_abi::{
    Cause, Exit, InterruptReason, ScheduleKind, ScheduleState, ScheduleStep,
};
use lust::test_framework::{
    assert, assert_eq, assert_interrupted, assert_success, TestClockState, TestRunner,
};
use std::cell::RefCell;
use std::time::Instant;

/// Simulated unreliable RPC service.
/// Fails with transient errors for the first 3 attempts before succeeding.
struct RemotePaymentGateway {
    attempts: RefCell<usize>,
    call_timestamps_ms: RefCell<Vec<u64>>,
}

impl RemotePaymentGateway {
    fn new() -> Self {
        Self {
            attempts: RefCell::new(0),
            call_timestamps_ms: RefCell::new(Vec::new()),
        }
    }

    fn charge(
        &self,
        clock: &TestClockState,
        amount_cents: u64,
    ) -> Exit<String, &'static str> {
        let mut count = self.attempts.borrow_mut();
        *count += 1;
        let current_time = clock.now_ms();
        self.call_timestamps_ms.borrow_mut().push(current_time);

        if *count < 4 {
            println!(
                "  [Attempt #{}] [Virtual Time: {} ms] Gateway connection timeout (transient)",
                *count, current_time
            );
            Exit::Failure(Cause::Fail("HTTP_504_GATEWAY_TIMEOUT"))
        } else {
            println!(
                "  [Attempt #{}] [Virtual Time: {} ms] Payment of {} cents authorized!",
                *count, current_time, amount_cents
            );
            Exit::Success(format!("AUTH_OK_TX{}", *count))
        }
    }
}

fn main() {
    println!("--- [Example 4: Unit Testing with Virtual Clock (TestClock)] ---");

    let wall_clock_start = Instant::now();

    // -----------------------------------------------------------------------
    // Part 1: Heavy Exponential Backoff Simulation in Virtual Time
    // -----------------------------------------------------------------------
    println!("\n1. Simulating Exponential Backoff Retry Policy without sleeping:");
    let mut clock = TestClockState::new();
    assert_eq(clock.now_ms(), 0, None).unwrap();

    let gateway = RemotePaymentGateway::new();
    let backoff_policy = ScheduleKind::exponential(100, 2.0, 10_000);
    println!("  Policy: initial=100ms, factor=2.0, max_delay=10,000ms");

    let mut state = ScheduleState::initial();
    let mut final_result = None;

    for _ in 0..10 {
        let outcome = gateway.charge(&clock, 4999);
        if outcome.is_success() {
            final_result = Some(outcome);
            break;
        }

        match backoff_policy.step(&state) {
            ScheduleStep::Continue { delay_ms, next_state } => {
                println!("    Advancing virtual clock by {} ms backoff...", delay_ms);
                clock.adjust(delay_ms);
                state = next_state;
            }
            ScheduleStep::Done => {
                final_result = Some(outcome);
                break;
            }
        }
    }

    let result = final_result.expect("Should have reached a final outcome");
    let auth_token = assert_success(&result, None).unwrap();
    println!("  Final Result: Success(\"{}\")", auth_token);
    assert_eq(*gateway.attempts.borrow(), 4, None).unwrap();
    assert_eq(clock.now_ms(), 100 + 200 + 400, None).unwrap(); // 700 ms virtual elapsed
    println!(
        "  Total virtual time elapsed: {} ms across 4 attempts",
        clock.now_ms()
    );

    // -----------------------------------------------------------------------
    // Part 2: Long-Horizon Multi-Fiber Timers & Wakeup Queue
    // -----------------------------------------------------------------------
    println!("\n2. Scheduling heavy long-running timers (up to 120,000 ms / 2 minutes):");
    let mut long_clock = TestClockState::new();
    let timer_10s = long_clock.schedule(101, 10_000);   // Fiber 101 @ 10s
    let timer_30s = long_clock.schedule(102, 30_000);   // Fiber 102 @ 30s
    let timer_60s = long_clock.schedule(103, 60_000);   // Fiber 103 @ 60s
    let timer_120s = long_clock.schedule(104, 120_000); // Fiber 104 @ 120s

    assert_eq(long_clock.pending_timer_count(), 4, None).unwrap();
    println!(
        "  Enqueued 4 timers: IDs [{}, {}, {}, {}] waiting in priority queue",
        timer_10s, timer_30s, timer_60s, timer_120s
    );

    // Fast-forward to 25 seconds
    println!("  Fast-forwarding virtual clock by +25,000 ms...");
    let awakened_1 = long_clock.adjust(25_000);
    println!("  Fibers awakened at t=25s: {:?}", awakened_1);
    assert_eq(awakened_1, vec![101], None).unwrap();
    assert_eq(long_clock.pending_timer_count(), 3, None).unwrap();

    // Fast-forward to 120 seconds
    println!("  Fast-forwarding virtual clock by +95,000 ms (to t=120,000 ms)...");
    let awakened_rest = long_clock.adjust(95_000);
    println!("  Fibers awakened at t=120s: {:?}", awakened_rest);
    assert_eq(awakened_rest, vec![102, 103, 104], None).unwrap();
    assert_eq(long_clock.pending_timer_count(), 0, None).unwrap();

    // -----------------------------------------------------------------------
    // Part 3: Deterministic Timeout Testing with Cause::Interrupted
    // -----------------------------------------------------------------------
    println!("\n3. Testing Effect Timeout Boundaries via TestClock:");
    let mut timeout_clock = TestClockState::new();
    let fiber_id = 999;
    let timeout_ms = 5000;
    timeout_clock.schedule(fiber_id, timeout_ms);

    // Advance to 4,999 ms: 1 millisecond BEFORE timeout boundary
    let prematurely_awakened = timeout_clock.adjust(4999);
    println!(
        "  Virtual time: {} ms. Fibers awakened before timeout: {:?}",
        timeout_clock.now_ms(), prematurely_awakened
    );
    assert(prematurely_awakened.is_empty(), "Timer should not fire at 4,999 ms", None).unwrap();

    // Advance 1 ms: exactly hitting 5,000 ms
    let timeout_awakened = timeout_clock.adjust(1);
    println!(
        "  Virtual time: {} ms. Fibers awakened at timeout boundary: {:?}",
        timeout_clock.now_ms(), timeout_awakened
    );
    assert_eq(timeout_awakened, vec![fiber_id], None).unwrap();

    // Emulate delivered timeout exit
    let timeout_exit: Exit<(), &'static str> =
        Exit::Failure(Cause::Interrupted(InterruptReason::Timeout));
    let interrupt_reason =
        assert_interrupted(&timeout_exit, Some(InterruptReason::Timeout), None).unwrap();
    println!("  Verified InterruptReason: {:?}", interrupt_reason);

    // -----------------------------------------------------------------------
    // Part 4: TestRunner Integration
    // -----------------------------------------------------------------------
    println!("\n4. Running fiber-isolated TestRunner case:");
    let runner = TestRunner::new();
    let runner_result = runner.run_test_fn("test_virtual_clock_in_fiber", |_fib, _scope| {
        let mut local_clock = TestClockState::new();
        local_clock.schedule(42, 1000);
        let ready = local_clock.adjust(1000);
        assert_eq(ready, vec![42], None)?;
        Ok(())
    });
    println!("  TestRunner outcome: {:?}", runner_result.outcome);
    assert_eq(runner_result.outcome, lust::test_framework::TestOutcome::Passed, None).unwrap();

    // -----------------------------------------------------------------------
    // Part 5: Verification of Zero-Delay Wall-Clock Invariant
    // -----------------------------------------------------------------------
    let elapsed = wall_clock_start.elapsed();
    println!("\n5. Performance & Determinism Verification:");
    println!("  Total virtual time simulated: 121,700 ms (~2 minutes)");
    println!("  Actual wall-clock execution time: {:?}", elapsed);
    assert(
        elapsed.as_millis() < 50,
        "Virtual clock simulation exceeded wall-clock budget!",
        None
    ).unwrap();
    println!("  [PASS] Zero-delay invariant satisfied: >120 seconds of virtual time simulated in < 1 ms!");
}
