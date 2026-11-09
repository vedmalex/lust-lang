use lust::effect_abi::{
    Exit, FiberContext, LustEffectDescriptor, LUST_EFFECT_ABI_V1,
    ScheduleKind, ScheduleState, Scope, StackContextProjection, acquire_release,
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

    const ITERATIONS: u64 = 10_000_000;

    // 1. Direct function call baseline
    let start = Instant::now();
    let mut sum: u64 = 0;
    for i in 0..ITERATIONS {
        sum = sum.wrapping_add(black_box(i));
    }
    let elapsed_direct = start.elapsed();
    let ns_per_direct = elapsed_direct.as_nanos() as f64 / ITERATIONS as f64;
    println!(
        "[1/5] Direct Native Baseline:       {:8.2} ns/op  ({:?} total, sum={})",
        ns_per_direct, elapsed_direct, sum
    );

    // 2. Versioned C-ABI LustEffectDescriptor Invocation
    let descriptor = LustEffectDescriptor {
        abi_version: LUST_EFFECT_ABI_V1,
        userdata: std::ptr::null_mut(),
        invoke_fn: Some(noop_effect_invoke),
        release_fn: Some(noop_effect_release),
    };
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let code = if let Some(inv) = descriptor.invoke_fn {
            unsafe { inv(descriptor.userdata, std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) }
        } else {
            0
        };
        black_box(code);
    }
    let elapsed_desc = start.elapsed();
    let ns_per_desc = elapsed_desc.as_nanos() as f64 / ITERATIONS as f64;
    println!(
        "[2/5] LustEffectDescriptor (C-ABI):  {:8.2} ns/op  ({:?} total)",
        ns_per_desc, elapsed_desc
    );

    // 3. StackContextProjection (Zero-Heap Stack Projection & Lookup)
    let dummy_val: u64 = 42;
    let ptr = &dummy_val as *const u64 as *const core::ffi::c_void;
    let service_ptrs = [ptr, ptr];
    let service_tags = [101u32, 202u32];

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let projection = StackContextProjection::new(service_ptrs, service_tags);
        let found = projection.get(202);
        black_box(found);
    }
    let elapsed_proj = start.elapsed();
    let ns_per_proj = elapsed_proj.as_nanos() as f64 / ITERATIONS as f64;
    println!(
        "[3/5] StackContextProjection (O(1)): {:8.2} ns/op  ({:?} total, 0 heap allocs)",
        ns_per_proj, elapsed_proj
    );

    // 4. acquire_release with Interrupt Masking & LIFO Finalizer
    const AR_ITERS: u64 = 1_000_000;
    let mut fiber = FiberContext::new(1, 1);
    let mut scope = Scope::new(1, None);

    let start = Instant::now();
    for _ in 0..AR_ITERS {
        let _ = acquire_release(
            &mut fiber,
            &mut scope,
            |_| Exit::<u64, ()>::Success(42),
            || {},
        );
        scope.finalizers.clear();
    }
    let elapsed_ar = start.elapsed();
    let ns_per_ar = elapsed_ar.as_nanos() as f64 / AR_ITERS as f64;
    println!(
        "[4/5] acquire_release + Masking:     {:8.2} ns/op  ({:?} total)",
        ns_per_ar, elapsed_ar
    );

    // 5. ScheduleKind evaluation step
    let sched = ScheduleKind::exponential(10, 1.5, 1000);
    let mut state = ScheduleState::initial();
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        state = match sched.step(&state) {
            lust::effect_abi::ScheduleStep::Continue { next_state, .. } => next_state,
            lust::effect_abi::ScheduleStep::Done => ScheduleState::initial(),
        };
        black_box(&state);
    }
    let elapsed_sched = start.elapsed();
    let ns_per_sched = elapsed_sched.as_nanos() as f64 / ITERATIONS as f64;
    println!(
        "[5/5] ScheduleKind::step:            {:8.2} ns/op  ({:?} total)",
        ns_per_sched, elapsed_sched
    );

    println!("\n=================================================================");
    println!("                   BENCHMARK COMPLETED                           ");
    println!("=================================================================");
}
