//! Example 1: Basic Effects, Exit and Cause handling in Lust.
//! Demonstrates pure effect blueprints, suspension, execution, and error handling.

use lust::effect_abi::{Cause, Exit, InterruptReason};

fn divide(a: i64, b: i64) -> Exit<i64, &'static str> {
    if b == 0 {
        Exit::Failure(Cause::Fail("division by zero"))
    } else {
        Exit::Success(a / b)
    }
}

fn compute_pipeline(x: i64) -> Exit<i64, &'static str> {
    // Pure function returning an Exit outcome composed via monadic and_then
    divide(100, x).and_then(|step1| divide(step1, 2))
}

fn main() {
    println!("--- [Example 1: Basic Effects & Exit] ---");

    // Successful execution
    match compute_pipeline(5) {
        Exit::Success(val) => println!("Success: 100 / 5 / 2 = {}", val),
        Exit::Failure(cause) => println!("Failed: {:?}", cause),
    }

    // Recoverable failure (Cause::Fail)
    match compute_pipeline(0) {
        Exit::Success(val) => println!("Success: {}", val),
        Exit::Failure(Cause::Fail(err)) => println!("Expected error caught: {}", err),
        Exit::Failure(other) => println!("Fatal or interrupt: {:?}", other),
    }

    // Terminal Interrupt (OutOfGas / Timeout) - fail-closed semantics
    let interrupted: Exit<i64, &'static str> =
        Exit::Failure(Cause::Interrupted(InterruptReason::OutOfGas));
    println!(
        "Interrupted status code: {} (is_interrupt: {})",
        interrupted.status_code(),
        interrupted.is_interrupt()
    );
}
