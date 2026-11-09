//! Example 2: Retries, Repeats and Schedule Policies in Lust.
//! Demonstrates Schedule algebra (recurs, exponential backoff, jitter) and fail-closed sticky interrupts.

use lust::effect_abi::{
    Cause, Exit, InterruptReason, ScheduleKind, repeat, retry, timeout,
};
use std::cell::RefCell;
use std::rc::Rc;

fn unreliable_network_call(attempts: &Rc<RefCell<usize>>) -> Exit<String, &'static str> {
    let mut count = attempts.borrow_mut();
    *count += 1;
    if *count < 3 {
        println!("  Attempt {}: Network timeout (transient)", *count);
        Exit::Failure(Cause::Fail("ECONNRESET"))
    } else {
        println!("  Attempt {}: Success!", *count);
        Exit::Success("Payload from server".to_string())
    }
}

fn fatal_oom_call() -> Exit<String, &'static str> {
    println!("  Executing effect with OutOfMemory...");
    Exit::Failure(Cause::Interrupted(InterruptReason::OutOfMemory))
}

fn main() {
    println!("--- [Example 2: Schedule & Retry Policies] ---");

    // 1. Retry with max 3 additional attempts
    let attempts = Rc::new(RefCell::new(0));
    let schedule = ScheduleKind::recurs(3);
    println!("Retrying unreliable call with Schedule::recurs(3):");
    let result = retry(|| unreliable_network_call(&attempts), &schedule);
    println!("Outcome: {:?}\n", result);

    // 2. Exponential backoff policy definition
    let exp_policy = ScheduleKind::exponential(50, 2.0, 1000).with_jitter();
    println!("Defined exponential jittered policy: {:?}", exp_policy);

    // 3. Fail-Closed Invariant: Interrupted causes NEVER retry!
    println!("\nTesting fail-closed invariant on OutOfMemory / OutOfGas:");
    let fatal_sched = ScheduleKind::recurs(10);
    let fatal_result = retry(fatal_oom_call, &fatal_sched);
    println!(
        "Fatal outcome: {:?} (zero additional retries attempted!)\n",
        fatal_result
    );

    // 4. Repeat on Success
    let counter = Rc::new(RefCell::new(0));
    println!("Repeating counter with Schedule::recurs(2):");
    let repeated = repeat(
        || {
            let mut c = counter.borrow_mut();
            *c += 1;
            println!("  Counter tick: {}", *c);
            Exit::<i32, &'static str>::Success(*c)
        },
        &ScheduleKind::recurs(2),
    );
    println!("Final repeat result: {:?}", repeated);

    // 5. Timeout wrapper
    let fast_task = timeout(|| Exit::<&str, &'static str>::Success("Quick answer"), 500);
    println!("\nTimeout check: {:?}", fast_task);
}
