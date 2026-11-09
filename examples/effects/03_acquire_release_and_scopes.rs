//! Example 3: Safe Resource Management with acquire_release and Structured Scopes.
//! Demonstrates interrupt masking during acquire, LIFO finalization, and the Gate-NEG-02 guarantee.

use lust::effect_abi::{
    Exit, FiberContext, InterruptReason, Scope, ScopeTeardown,
    acquire_release,
};
use std::cell::RefCell;
use std::rc::Rc;

#[allow(dead_code)]
struct FileHandle {
    fd: u32,
    open: Rc<RefCell<bool>>,
}

impl FileHandle {
    fn open(fd: u32, tracker: Rc<RefCell<bool>>) -> Self {
        println!("  [Resource] Opened file descriptor #{}", fd);
        *tracker.borrow_mut() = true;
        Self { fd, open: tracker }
    }
}

fn main() {
    println!("--- [Example 3: acquire_release & Structured Scopes] ---");

    let mut fiber = FiberContext::new(101, 1);
    let mut scope = Scope::new(1, None);
    let file_open_tracker = Rc::new(RefCell::new(false));

    println!("1. Executing acquire_release with interrupt masking:");
    let tracker_clone = file_open_tracker.clone();
    let tracker_for_cleanup = file_open_tracker.clone();
    let res = acquire_release(
        &mut fiber,
        &mut scope,
        |fib| {
            let handle = FileHandle::open(42, tracker_clone);

            // Simulating an interrupt (e.g. cancellation signal) arriving mid-acquisition:
            let delivered = fib.request_interrupt(InterruptReason::Cancelled);
            println!(
                "  [Interrupt] Cancellation signal arrived during acquire. Delivered immediately? {}",
                delivered
            );
            println!(
                "  [Interrupt] Is fiber currently masked? {}",
                fib.is_masked()
            );

            Exit::<u32, &'static str>::Success(handle.fd)
        },
        move || {
            *tracker_for_cleanup.borrow_mut() = false;
            println!("  [Cleanup] Executing finalizer: Closing file descriptor #42");
        },
    );

    println!(
        "Acquire phase outcome (deferred interrupt delivered upon unmasking): {:?}",
        res
    );
    assert_eq!(
        res,
        Exit::Failure(lust::effect_abi::Cause::Interrupted(
            InterruptReason::Cancelled
        ))
    );
    println!("Finalizers registered in Scope: {}", scope.finalizers.len());

    println!("\n2. Scope Teardown Invariant (Gate-NEG-02):");
    println!("  Closing parent scope triggers LIFO finalization of all acquired resources:");
    scope.finalize_resources();
    assert_eq!(scope.teardown, ScopeTeardown::Done);
    assert_eq!(*file_open_tracker.borrow(), false);
    println!("  Scope teardown completed successfully! Resource open state: {}", *file_open_tracker.borrow());
}
