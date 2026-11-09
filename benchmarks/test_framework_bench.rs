use lust::{
    effect_abi::{Cause, Exit, InterruptReason, ScheduleKind, StackContextProjection},
    test_framework::{
        assert, assert_almost_eq, assert_eq, assert_fails, assert_interrupted,
        assert_ne, assert_success, MockService, Reporter, ReporterKind, TestClockState,
        TestRunner, TestSuiteResult,
    },
};
use std::hint::black_box;
use std::time::Instant;

fn main() {
    println!("=================================================================");
    println!("       LUST TESTING FRAMEWORK (@std/test) PERFORMANCE BENCHMARK   ");
    println!("=================================================================\n");

    const ITERS_FAST: u64 = 10_000_000;
    const ITERS_MID: u64 = 1_000_000;
    const ITERS_SLOW: u64 = 100_000;

    // =========================================================================
    // 1. PURE VALUE ASSERTIONS
    // =========================================================================
    println!("--- [1. Pure Value Assertions] ---");

    // 1.1 assert(true)
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert(black_box(true), "should pass", None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  1.1 assert(bool=true):                     {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 1.2 assert_eq (passing integers)
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert_eq(black_box(42), black_box(42), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  1.2 assert_eq (passing i32):               {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 1.3 assert_eq (passing strings)
    let s1 = "user_auth_token_xyz_123456789";
    let s2 = "user_auth_token_xyz_123456789";
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert_eq(black_box(s1), black_box(s2), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  1.3 assert_eq (passing &str):              {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 1.4 assert_eq (failing with diff formatting)
    let start = Instant::now();
    for _ in 0..ITERS_SLOW {
        let r = assert_eq(black_box(42), black_box(99), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  1.4 assert_eq (failing + format_diff):     {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_SLOW as f64,
        elapsed
    );

    // 1.5 assert_ne (passing)
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert_ne(black_box(1), black_box(2), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  1.5 assert_ne (passing):                   {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 1.6 assert_almost_eq (floats)
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert_almost_eq(black_box(1.00001), black_box(1.00002), 0.001, None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  1.6 assert_almost_eq (f64 epsilon):        {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // =========================================================================
    // 2. EFFECT SYSTEM OUTCOME ASSERTIONS
    // =========================================================================
    println!("\n--- [2. First-Class Effect System Assertions] ---");

    // 2.1 assert_success
    let ok_exit: Exit<u64, &'static str> = Exit::Success(42);
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert_success(black_box(&ok_exit), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  2.1 assert_success (Exit::Success):        {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 2.2 assert_fails (typed fail)
    let fail_exit: Exit<u64, &'static str> = Exit::Failure(Cause::Fail("ECONNREFUSED"));
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert_fails(black_box(&fail_exit), Some(&"ECONNREFUSED"), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  2.2 assert_fails (typed Cause::Fail):      {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 2.3 assert_interrupted
    let int_exit: Exit<u64, &'static str> =
        Exit::Failure(Cause::Interrupted(InterruptReason::Timeout));
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let r = assert_interrupted(black_box(&int_exit), Some(InterruptReason::Timeout), None);
        let _ = black_box(r);
    }
    let elapsed = start.elapsed();
    println!(
        "  2.3 assert_interrupted (InterruptReason):  {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // =========================================================================
    // 3. VIRTUAL TIME ENGINE (TestClock) VS PHYSICAL TIME
    // =========================================================================
    println!("\n--- [3. Virtual Time Engine (TestClock) vs Physical Time] ---");

    // 3.1 Timer Schedule in BinaryHeap
    let mut clock = TestClockState::new();
    let start = Instant::now();
    for i in 0..ITERS_MID {
        let tid = clock.schedule(i, 100);
        black_box(tid);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.1 TestClock.schedule (BinaryHeap push):  {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_MID as f64,
        elapsed
    );

    // 3.2 Warp Time: Single timer adjustment
    let start = Instant::now();
    for _ in 0..ITERS_MID {
        let mut clk = TestClockState::new();
        clk.schedule(101, 500);
        let awakened = clk.adjust(500);
        black_box(awakened);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.2 TestClock.adjust (warp 1 timer / 500ms):{:8.2} ns/op ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_MID as f64,
        elapsed
    );

    // 3.3 Warp Time: 10 scheduled timers awakened simultaneously
    let start = Instant::now();
    for _ in 0..ITERS_SLOW {
        let mut clk = TestClockState::new();
        for i in 1..=10 {
            clk.schedule(i, i * 100);
        }
        let awakened = clk.adjust(1000);
        black_box(awakened);
    }
    let elapsed = start.elapsed();
    let ns_per_warp_10 = elapsed.as_nanos() as f64 / ITERS_SLOW as f64;
    println!(
        "  3.3 TestClock.adjust (warp 10 timers/1000ms):{:7.2} ns/op ({:?} total)",
        ns_per_warp_10, elapsed
    );

    // 3.4 simulate_schedule (Exponential 10 steps in virtual time)
    let exp_policy = ScheduleKind::exponential(10, 1.5, 5000);
    let start = Instant::now();
    for _ in 0..ITERS_SLOW {
        let mut clk = TestClockState::new();
        let stats = clk.simulate_schedule(&exp_policy, 10);
        black_box(stats);
    }
    let elapsed = start.elapsed();
    println!(
        "  3.4 simulate_schedule (10 exp steps warp): {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_SLOW as f64,
        elapsed
    );

    // Physical time comparison
    println!("  3.5 Physical vs Virtual Speedup Factor:");
    println!(
        "      -> 1000ms of simulated time processed in {:.2} ns (~{:.2e}x real-time)",
        ns_per_warp_10,
        1_000_000_000.0 / ns_per_warp_10
    );

    // =========================================================================
    // 4. MOCK SERVICE & CALL RECORDING
    // =========================================================================
    println!("\n--- [4. MockService & Call Spy Tracking] ---");

    const ITERS_MOCK: u64 = 50_000;
    let mock = MockService::new(101);
    let start = Instant::now();
    for i in 0..ITERS_MOCK {
        mock.record_call("find_user", &["user_1234"], i);
    }
    let elapsed = start.elapsed();
    println!(
        "  4.1 MockService.record_call:               {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_MOCK as f64,
        elapsed
    );

    let small_mock = MockService::new(202);
    for i in 0..50 {
        small_mock.record_call("find_user", &["user_1234"], i);
    }

    let start = Instant::now();
    for _ in 0..ITERS_MOCK {
        let cnt = small_mock.call_count("find_user");
        black_box(cnt);
    }
    let elapsed = start.elapsed();
    println!(
        "  4.2 MockService.call_count (50 entries):   {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_MOCK as f64,
        elapsed
    );

    let start = Instant::now();
    for _ in 0..ITERS_MOCK {
        let last = small_mock.last_call("find_user");
        black_box(last);
    }
    let elapsed = start.elapsed();
    println!(
        "  4.3 MockService.last_call (50 entries):    {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_MOCK as f64,
        elapsed
    );

    // =========================================================================
    // 5. ZERO-HEAP STACKCONTEXTPROJECTION<N>
    // =========================================================================
    println!("\n--- [5. Zero-Heap StackContextProjection<N>] ---");

    let mock1 = MockService::new(101);
    let mock2 = MockService::new(102);
    let mock3 = MockService::new(103);
    let mock4 = MockService::new(104);

    // 5.1 Single projection (N=1)
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let proj = mock1.to_projection_single();
        let ptr = proj.get(101);
        black_box(ptr);
    }
    let elapsed = start.elapsed();
    println!(
        "  5.1 StackContextProjection<1> from Mock:   {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 5.2 Pair projection (N=2)
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let proj = MockService::to_projection_pair(&mock1, &mock2);
        let ptr = proj.get(102);
        black_box(ptr);
    }
    let elapsed = start.elapsed();
    println!(
        "  5.2 StackContextProjection<2> from Mocks:  {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // 5.3 Quad projection (N=4)
    let (p1, t1) = mock1.as_projection_entry();
    let (p2, t2) = mock2.as_projection_entry();
    let (p3, t3) = mock3.as_projection_entry();
    let (p4, t4) = mock4.as_projection_entry();
    let start = Instant::now();
    for _ in 0..ITERS_FAST {
        let proj = StackContextProjection::new([p1, p2, p3, p4], [t1, t2, t3, t4]);
        let ptr = proj.get(104);
        black_box(ptr);
    }
    let elapsed = start.elapsed();
    println!(
        "  5.3 StackContextProjection<4> Resolution:  {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        elapsed.as_nanos() as f64 / ITERS_FAST as f64,
        elapsed
    );

    // =========================================================================
    // 6. FIBER-ISOLATED RUNNER & PANIC CONTAINMENT
    // =========================================================================
    println!("\n--- [6. Fiber-Isolated TestRunner Execution & Containment] ---");

    let runner = TestRunner::new();

    // 6.1 Passing test execution overhead
    let start = Instant::now();
    for _ in 0..ITERS_SLOW {
        let res = runner.run_test_fn("passing_bench", |_fib, _scope| Ok(()));
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  6.1 TestRunner.run_test_fn (passing):      {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_SLOW as f64,
        elapsed
    );

    // 6.2 Failing test execution overhead
    let start = Instant::now();
    for _ in 0..ITERS_SLOW {
        let res = runner.run_test_fn("failing_bench", |_fib, _scope| {
            assert_eq(1, 2, None)
        });
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  6.2 TestRunner.run_test_fn (assertion fail):{:7.2} ns/op ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_SLOW as f64,
        elapsed
    );

    // 6.3 Interruption containment (Gate-TEST-02 unhandled interrupt in fiber)
    const INT_ITERS: u64 = 100_000;
    let start = Instant::now();
    for _ in 0..INT_ITERS {
        let res = runner.run_test_fn("interrupted_bench", |fib, _scope| {
            fib.pending_interrupt = Some(InterruptReason::OutOfGas);
            Ok(())
        });
        black_box(res);
    }
    let elapsed = start.elapsed();
    println!(
        "  6.3 TestRunner.run_test_fn (OutOfGas contain):{:7.2} ns/op ({:?} total)",
        elapsed.as_nanos() as f64 / INT_ITERS as f64,
        elapsed
    );

    // 6.4 Reporter generation
    let suite = TestSuiteResult {
        name: "Micro Suite".into(),
        tests: vec![
            runner.run_test_fn("t1", |_f, _s| Ok(())),
            runner.run_test_fn("t2", |_f, _s| Ok(())),
        ],
        nested_suites: Vec::new(),
        duration: std::time::Duration::from_millis(1),
    };
    let summary = TestRunner::summarize(&[suite.clone()]);
    let start = Instant::now();
    for _ in 0..ITERS_SLOW {
        let tap = Reporter::format(ReporterKind::Tap, &[suite.clone()], &summary);
        black_box(tap);
    }
    let elapsed = start.elapsed();
    println!(
        "  6.4 Reporter::format (TAP v13 emit):       {:8.2} ns/op  ({:?} total)",
        elapsed.as_nanos() as f64 / ITERS_SLOW as f64,
        elapsed
    );

    println!("\n=================================================================");
    println!("                   BENCHMARK COMPLETED                           ");
    println!("=================================================================");
}
