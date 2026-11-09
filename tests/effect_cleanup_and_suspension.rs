#![cfg(feature = "std")]

use lust::{EmbeddedProgram, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

fn run_vm(source: &str, func: &str) -> lust::Result<Value> {
    let mut prog = EmbeddedProgram::builder()
        .module("main", source)
        .entry_module("main")
        .compile()?;
    prog.vm_mut().call(&format!("main.{}", func), vec![])
}

#[test]
fn test_defer_lifo_order_and_return_isolation() {
    let src = r#"
        local state = 0

        function defer_order(): int
            state = 1
            defer state = state * 2
            defer state = state + 10
            return state
        end

        function get_state(): int
            return state
        end
    "#;
    let mut prog = EmbeddedProgram::builder()
        .module("main", src)
        .entry_module("main")
        .compile()
        .expect("compile");

    // return value evaluated before defers
    let ret = prog.vm_mut().call("main.defer_order", vec![]).expect("call defer_order");
    assert_eq!(ret, Value::Int(1));

    // defers executed in LIFO order: (1 + 10) * 2 = 22 (FIFO would be (1 * 2) + 10 = 12)
    let state = prog.vm_mut().call("main.get_state", vec![]).expect("call get_state");
    assert_eq!(state, Value::Int(22));
}

#[test]
fn test_scoped_defer_boundary() {
    let src = r#"
        local log = 0

        function test_scoped(): int
            log = 1
            scoped do
                defer log = log + 100
                log = log + 10
            end
            -- after scoped block, defer must have executed
            return log
        end
    "#;
    let res = run_vm(src, "test_scoped").expect("run");
    assert_eq!(res, Value::Int(111));
}

static CLEANUP_SUM: AtomicUsize = AtomicUsize::new(0);

fn mock_cleanup(handle: usize) {
    CLEANUP_SUM.fetch_add(handle, Ordering::SeqCst);
}

#[test]
fn test_trusted_host_resource_ledger_capacity_and_vm_drop() {
    CLEANUP_SUM.store(0, Ordering::SeqCst);

    let mut vm = lust::VM::new();

    // Acquire up to 256 resources
    let mut slots = Vec::new();
    for i in 1..=256 {
        let slot = vm.acquire_host_resource(i, mock_cleanup).expect("acquire slot");
        slots.push(slot);
    }
    assert_eq!(vm.host_ledger.count, 256);

    // 257th acquire must fail-closed with TooManyOpenResources
    let err = vm.acquire_host_resource(999, mock_cleanup).unwrap_err();
    let err_msg = format!("{}", err);
    assert!(
        err_msg.contains("TooManyOpenResources"),
        "Expected TooManyOpenResources, got: {}",
        err_msg
    );

    // Release slot 0 (handle 1)
    vm.release_host_resource(slots[0]);
    assert_eq!(CLEANUP_SUM.load(Ordering::SeqCst), 1);
    assert_eq!(vm.host_ledger.count, 255);

    // Idempotent double release of same slot: must not double-cleanup
    vm.release_host_resource(slots[0]);
    assert_eq!(CLEANUP_SUM.load(Ordering::SeqCst), 1);

    // Drop the VM: remaining 255 slots (handles 2..=256) must be cleaned up in Drop for VM
    drop(vm);

    // Sum of 1..=256 is 256 * 257 / 2 = 32896
    let expected_sum = (256 * 257) / 2;
    assert_eq!(CLEANUP_SUM.load(Ordering::SeqCst), expected_sum);
}

#[test]
fn test_loop_iteration_defer_cleanup() {
    let src = r#"
        local sum = 0

        function test_loop(): int
            sum = 0
            local i = 0
            while i < 3 do
                defer sum = sum + 1
                i = i + 1
            end
            return sum
        end
    "#;
    let res = run_vm(src, "test_loop").expect("run");
    assert_eq!(res, Value::Int(3));
}

#[test]
fn test_errdefer_dormant_on_normal_exit() {
    let src = r#"
        local flag = 0

        function test_errdefer(): int
            flag = 10
            errdefer flag = 999
            defer flag = flag + 5
            return flag
        end

        function get_flag(): int
            return flag
        end
    "#;
    let mut prog = EmbeddedProgram::builder()
        .module("main", src)
        .entry_module("main")
        .compile()
        .expect("compile");

    let ret = prog.vm_mut().call("main.test_errdefer", vec![]).expect("call");
    assert_eq!(ret, Value::Int(10));

    // errdefer must NOT have run, only defer ran (flag is 10 + 5 = 15, not 999)
    let flag = prog.vm_mut().call("main.get_flag", vec![]).expect("call get_flag");
    assert_eq!(flag, Value::Int(15));
}
