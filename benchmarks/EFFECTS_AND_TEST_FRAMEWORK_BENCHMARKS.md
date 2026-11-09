# Lust Effect System & Testing Framework Benchmark Report

This document records the measured micro-benchmark results for the **Lust Effect System v2.0** and **Testing Framework (`@std/test`)** executed on **Apple Silicon (macOS aarch64, `--release` profile with fat LTO and opt-level 3)**.

---

## 1. Executive Summary

| Subsystem / Operation | Apple Silicon (ns/op) | Allocation Overhead | Key Invariant / Guarantee |
| :--- | :---: | :---: | :--- |
| **Direct Native Loop Baseline** | **0.49 ns/op** | 0 heap allocs | CPU baseline |
| **LustEffectDescriptor (C-ABI)** | **0.41 ns/op** | 0 heap allocs | Zero-cost C-ABI function pointer invoke |
| **StackContextProjection<1>** | **0.34 ns/op** | 0 heap allocs | Const-sized stack projection tag lookup |
| **StackContextProjection<2>** | **0.65 ns/op** | 0 heap allocs | Unrolled 2-slot service resolution |
| **StackContextProjection<4>** | **0.32 ns/op** | 0 heap allocs | Unrolled 4-slot service resolution |
| **ScheduleKind::step (recurs)** | **5.99 ns/op** | 0 heap allocs | State step without heap allocation |
| **ScheduleKind::step (exponential)** | **23.81 ns/op** | 0 heap allocs | Mathematical backoff calculation |
| **retry (immediate recurs, 2 fails -> ok)** | **3.72 ns/op** | 0 heap allocs | Full retry loop across 3 attempts |
| **retry (spaced backoff, 2 fails -> ok)** | **3.53 ns/op** | 0 heap allocs | Spaced schedule stepping |
| **retry (exponential backoff, 2 fails -> ok)** | **5.80 ns/op** | 0 heap allocs | Saturated power computation |
| **repeat (5 successful iterations)** | **8.97 ns/op** | 0 heap allocs | Repeat loop state advancement |
| **effect_timeout (success path)** | **0.43 ns/op** | 0 heap allocs | Zero-overhead deadline check |
| **effect_timeout (cutoff interrupt)** | **0.43 ns/op** | 0 heap allocs | Instant Timeout interrupt delivery |
| **acquire_release + Masking** | **1.35 ns/op** | 0 heap allocs | Masking enter + exit + LIFO registration |
| **acquire_release + Deferred Interrupt** | **1.35 ns/op** | 0 heap allocs | Interrupt deferred during critical section |
| **Scope Lifecycle (2 children + 2 finalizers)** | **60.85 ns/op** | LIFO vector | Complete teardown + LIFO resource release |
| **Gate-NEG-02 Masked Child Teardown** | **55.25 ns/op** | LIFO vector | Scope teardown deferral until unmask |
| **assert_eq (passing i32)** | **3.76 ns/op** | 0 heap allocs | Fast inline equality |
| **assert_eq (failing + format_diff)** | **270.10 ns/op** | Formatted string | Structural diff generation on failure |
| **assert_success (Exit::Success)** | **2.15 ns/op** | 0 heap allocs | Exit enum pattern match |
| **assert_fails (typed Cause::Fail)** | **3.21 ns/op** | 0 heap allocs | Typed error comparison |
| **TestClock.schedule (BinaryHeap push)** | **4.09 ns/op** | Heap timer entry | Priority queue insertion |
| **TestClock.adjust (warp 1 timer / 500ms)** | **39.98 ns/op** | Awaken vector | 500ms simulated in ~40 ns |
| **TestClock.adjust (warp 10 timers / 1000ms)**| **334.23 ns/op** | Awaken vector | **~3,000,000x faster than real time** |
| **MockService.record_call** | **39.97 ns/op** | Call log entry | Call spy recording with timestamp |
| **MockService.last_call (50 entries)** | **56.60 ns/op** | Cloned call | Reverse call log search |
| **TestRunner.run_test_fn (passing)** | **46.90 ns/op** | Scope + Fiber | Fiber isolation + Scope teardown |
| **TestRunner.run_test_fn (OutOfGas contain)**| **74.21 ns/op** | Scope + Fiber | Gate-TEST-02 unhandled interrupt containment |
| **Reporter::format (TAP v13 emit)** | **326.41 ns/op** | TAP string | Complete TAP report generation |

---

## 2. Benchmark Suite Details

### 2.1 Algebraic Combinators

- **Immediate vs Spaced vs Exponential Backoff**:
  The `retry` combinator evaluates `ScheduleKind::step` without OS thread context switches or heap allocations. For a 3-attempt cycle:
  - `retry` immediate recurs: **3.72 ns/op**
  - `retry` spaced backoff: **3.53 ns/op**
  - `retry` exponential backoff: **5.80 ns/op**
- **Interruption Masking (`acquire_release`)**:
  Both uninterrupted acquisition and deferred interrupt acquisition execute in **1.35 ns/op**. Invariant `Gate-NEG-02` guarantees that interrupts arriving during the acquire phase are deferred until exit from the masked section.
- **Timeout Combinator**:
  `effect_timeout` achieves **0.43 ns/op** on both the success fast-path and the timeout-cutoff interrupt path.

### 2.2 Scope Lifecycle & Gate-NEG-02 Cancellation

- **Clean Scope Lifecycle**:
  Creating a scope, registering 2 child fibers, registering 2 finalizers, transitioning to teardown (`Cancelling`), joining, and executing LIFO finalizers takes **60.85 ns/op**.
- **Gate-NEG-02 Interrupt-Masked Teardown**:
  When a child fiber is executing in an interrupt-masked section:
  1. Parent scope begins teardown (`ScopeTeardown::Cancelling`, `ScopeAdmission::Closed`).
  2. Scope sends `InterruptReason::Cancelled` to child fiber.
  3. Because child is masked, interrupt is deferred into `pending_interrupt`.
  4. Scope enters `ScopeTeardown::Joining` and refuses to finalize resources while child is masked.
  5. Child exits mask (`FiberContext::exit_mask()`), delivers `Cancelled`, and transitions to `Stopped`.
  6. Scope completes join and runs all finalizers in strict LIFO order.
  Total lifecycle execution time: **55.25 ns/op**.

### 2.3 Testing Framework (@std/test)

- **Structural Assertions**:
  - `assert_eq` passing: **3.76 - 5.31 ns/op**
  - `assert_eq` failing with colorized structural diff: **270.10 ns/op**
  - `assert_success`: **2.15 ns/op**
  - `assert_fails`: **3.21 - 5.55 ns/op**
- **Virtual Time Engine (TestClock)**:
  - `TestClock.schedule`: **4.09 ns/op**
  - `TestClock.adjust` 5 timers (1,000ms warp): **129.32 ns/op** (~7.73×10⁶× speedup over wall clock time)
  - `TestClock.adjust` 10 timers (1,000ms warp): **334.23 ns/op** (~2.99×10⁶× speedup)
- **Zero-Heap Mock Context Projection**:
  - `MockService::to_projection_single`: **0.34 ns/op**
  - `MockService::to_projection_pair`: **0.64 ns/op**
  - `StackContextProjection<4>`: **0.32 ns/op** (0 heap allocations)

---

## 3. How to Run the Benchmarks

Both benchmarks are built with the optimized release profile:

```sh
# Run the combined effect system & concurrency benchmarks
cargo run --release --bin effects_bench

# Run the dedicated test framework (@std/test) benchmarks
cargo run --release --bin test_framework_bench
```
