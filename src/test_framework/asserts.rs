//! Assertions Engine for pure values and Effect System v2.0 outcomes.

use alloc::{format, string::String};
use crate::ast::Span;
use crate::effect_abi::{Cause, Exit, InterruptReason};
use super::diff::format_diff;

/// Structured failure reported by an assertion.
#[derive(Debug, Clone, PartialEq)]
pub struct AssertError {
    pub message: String,
    pub location: Option<Span>,
    pub diff: Option<String>,
    pub fiber_id: Option<u64>,
}

impl core::fmt::Display for AssertError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Assertion failed: {}", self.message)?;
        if let Some(loc) = self.location {
            write!(f, " at {}:{}:{}:{}", loc.start_line, loc.start_col, loc.end_line, loc.end_col)?;
        }
        if let Some(fib) = self.fiber_id {
            write!(f, " (fiber #{})", fib)?;
        }
        if let Some(diff) = &self.diff {
            write!(f, "\n{}", diff)?;
        }
        Ok(())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AssertError {}

// ---------------------------------------------------------------------------
// Pure Value Assertions
// ---------------------------------------------------------------------------

/// Asserts that a boolean condition is true.
pub fn assert(condition: bool, message: &str, span: Option<Span>) -> Result<(), AssertError> {
    if !condition {
        Err(AssertError {
            message: message.into(),
            location: span,
            diff: None,
            fiber_id: None,
        })
    } else {
        Ok(())
    }
}

/// Asserts equality between two values with detailed diff.
pub fn assert_eq<T: PartialEq + core::fmt::Debug>(actual: T, expected: T, span: Option<Span>) -> Result<(), AssertError> {
    if actual != expected {
        let diff = format_diff(&actual, &expected);
        Err(AssertError {
            message: "Values are not equal".into(),
            location: span,
            diff: Some(diff),
            fiber_id: None,
        })
    } else {
        Ok(())
    }
}

/// Asserts inequality between two values.
pub fn assert_ne<T: PartialEq + core::fmt::Debug>(actual: T, expected: T, span: Option<Span>) -> Result<(), AssertError> {
    if actual == expected {
        Err(AssertError {
            message: format!("Values were expected to be different, but both are: {:?}", actual),
            location: span,
            diff: None,
            fiber_id: None,
        })
    } else {
        Ok(())
    }
}

/// Asserts that two floating point numbers are within epsilon tolerance.
pub fn assert_almost_eq(actual: f64, expected: f64, epsilon: f64, span: Option<Span>) -> Result<(), AssertError> {
    let diff = (actual - expected).abs();
    if diff > epsilon {
        Err(AssertError {
            message: format!(
                "Floats not within epsilon={}: actual={}, expected={}, delta={}",
                epsilon, actual, expected, diff
            ),
            location: span,
            diff: None,
            fiber_id: None,
        })
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// First-Class Effect System Assertions
// ---------------------------------------------------------------------------

/// Asserts that an effect exited successfully (`Exit::Success(val)`).
/// Returns a reference to the successful value.
pub fn assert_success<'a, A: core::fmt::Debug, E: core::fmt::Debug>(
    exit: &'a Exit<A, E>,
    span: Option<Span>,
) -> Result<&'a A, AssertError> {
    match exit {
        Exit::Success(val) => Ok(val),
        Exit::Failure(cause) => Err(AssertError {
            message: format!("Expected Exit::Success, but effect failed with: {:?}", cause),
            location: span,
            diff: None,
            fiber_id: None,
        }),
    }
}

/// Asserts that an effect failed with an expected recoverable typed error (`Cause::Fail(e)`).
/// If `expected_err` is provided, verifies that `e == expected_err`.
pub fn assert_fails<'a, A: core::fmt::Debug, E: PartialEq + core::fmt::Debug>(
    exit: &'a Exit<A, E>,
    expected_err: Option<&E>,
    span: Option<Span>,
) -> Result<&'a E, AssertError> {
    match exit {
        Exit::Failure(Cause::Fail(err)) => {
            if let Some(expected) = expected_err {
                if err != expected {
                    return Err(AssertError {
                        message: "Effect failed with different error than expected".into(),
                        location: span,
                        diff: Some(format_diff(err, expected)),
                        fiber_id: None,
                    });
                }
            }
            Ok(err)
        }
        Exit::Success(val) => Err(AssertError {
            message: format!("Expected Exit::Failure(Cause::Fail), but effect succeeded with: {:?}", val),
            location: span,
            diff: None,
            fiber_id: None,
        }),
        Exit::Failure(other) => Err(AssertError {
            message: format!("Expected typed Cause::Fail, but got other failure: {:?}", other),
            location: span,
            diff: None,
            fiber_id: None,
        }),
    }
}

/// Asserts that an effect was terminated by an interrupt (`Cause::Interrupted(reason)`).
/// If `expected_reason` is specified, verifies that the reason matches (e.g. `OutOfGas` vs `Timeout`).
pub fn assert_interrupted<A, E: core::fmt::Debug>(
    exit: &Exit<A, E>,
    expected_reason: Option<InterruptReason>,
    span: Option<Span>,
) -> Result<InterruptReason, AssertError> {
    match exit {
        Exit::Failure(Cause::Interrupted(reason)) => {
            if let Some(expected) = expected_reason {
                if *reason != expected {
                    return Err(AssertError {
                        message: format!(
                            "Interrupted with different reason: actual={:?}, expected={:?}",
                            reason, expected
                        ),
                        location: span,
                        diff: None,
                        fiber_id: None,
                    });
                }
            }
            Ok(*reason)
        }
        Exit::Failure(Cause::Interrupt) => {
            let reason = InterruptReason::Cancelled;
            if let Some(expected) = expected_reason {
                if reason != expected {
                    return Err(AssertError {
                        message: format!("Generic interrupt received, expected: {:?}", expected),
                        location: span,
                        diff: None,
                        fiber_id: None,
                    });
                }
            }
            Ok(reason)
        }
        Exit::Success(_) => Err(AssertError {
            message: "Expected effect to be interrupted, but it succeeded".into(),
            location: span,
            diff: None,
            fiber_id: None,
        }),
        Exit::Failure(other) => Err(AssertError {
            message: format!("Expected Cause::Interrupted, but got: {:?}", other),
            location: span,
            diff: None,
            fiber_id: None,
        }),
    }
}

/// Asserts that an effect died from an untyped defect / panic (`Cause::Die`).
pub fn assert_dies<'a, A, E: core::fmt::Debug>(
    exit: &'a Exit<A, E>,
    expected_panic: Option<&str>,
    span: Option<Span>,
) -> Result<&'a str, AssertError> {
    match exit {
        Exit::Failure(Cause::Die(payload)) => {
            let s = payload.as_str();
            if let Some(expected) = expected_panic {
                if !s.contains(expected) {
                    return Err(AssertError {
                        message: format!("Panic message \"{}\" does not contain expected substring \"{}\"", s, expected),
                        location: span,
                        diff: None,
                        fiber_id: None,
                    });
                }
            }
            Ok(s)
        }
        Exit::Success(_) => Err(AssertError {
            message: "Expected effect to die with panic, but it succeeded".into(),
            location: span,
            diff: None,
            fiber_id: None,
        }),
        Exit::Failure(other) => Err(AssertError {
            message: format!("Expected Cause::Die, but got: {:?}", other),
            location: span,
            diff: None,
            fiber_id: None,
        }),
    }
}
