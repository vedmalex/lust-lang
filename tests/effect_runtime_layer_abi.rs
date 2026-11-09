#![cfg(feature = "std")]

use lust::effect_abi::{
    Cause, Exit, LayerRollbackLedger, LustClosureObject, LustEffectDescriptor,
    StackContextProjection, LUST_EFFECT_ABI_V1, LUST_STATUS_ERR_GENERIC,
    LUST_STATUS_ERR_INTERRUPTED, LUST_STATUS_ERR_OUT_OF_GAS,
    LUST_STATUS_ERR_OUT_OF_MEMORY, LUST_STATUS_SUCCESS,
};
use lust::{EmbeddedProgram, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn test_c_abi_effect_descriptor_layout_and_version() {
    let desc = LustEffectDescriptor::default();
    assert_eq!(desc.abi_version, LUST_EFFECT_ABI_V1);
    assert_eq!(desc.abi_version, 0x0001_0000);
    assert!(desc.userdata.is_null());
    assert!(desc.invoke_fn.is_none());
    assert!(desc.release_fn.is_none());

    assert_eq!(LUST_STATUS_SUCCESS, 0);
    assert_eq!(LUST_STATUS_ERR_GENERIC, 1);
    assert_eq!(LUST_STATUS_ERR_OUT_OF_GAS, 2);
    assert_eq!(LUST_STATUS_ERR_OUT_OF_MEMORY, 3);
    assert_eq!(LUST_STATUS_ERR_INTERRUPTED, 4);

    let closure = LustClosureObject {
        code_ptr: std::ptr::null(),
        env_layout_id: 42,
        capture_handle: std::ptr::null_mut(),
    };
    assert_eq!(closure.env_layout_id, 42);
}

#[test]
fn test_exit_and_cause_sticky_interrupt_semantics() {
    // Gate-NEG-01 invariant: Interrupt cannot be swallowed by catch
    let exit_success: Exit<i32, String> = Exit::Success(42);
    assert!(exit_success.is_success());
    assert!(!exit_success.is_failure());
    assert!(!exit_success.is_interrupt());
    assert_eq!(exit_success.status_code(), LUST_STATUS_SUCCESS);

    let exit_fail: Exit<i32, String> = Exit::Failure(Cause::Fail("recoverable".into()));
    assert!(exit_fail.is_failure());
    assert!(!exit_fail.is_interrupt());
    assert_eq!(exit_fail.status_code(), LUST_STATUS_ERR_GENERIC);

    // Expected failure can be caught and recovered
    let recovered = exit_fail.catch(|err| {
        assert_eq!(err, "recoverable");
        Exit::Success(99)
    });
    assert_eq!(recovered, Exit::Success(99));

    // Sticky Interrupt CANNOT be caught
    let exit_interrupt: Exit<i32, String> = Exit::Failure(Cause::Interrupt);
    assert!(exit_interrupt.is_interrupt());
    assert_eq!(exit_interrupt.status_code(), LUST_STATUS_ERR_INTERRUPTED);

    let attempted_catch = exit_interrupt.clone().catch(|_| Exit::Success(42));
    // Must remain Interrupt!
    assert!(attempted_catch.is_interrupt());
    assert_eq!(attempted_catch, Exit::Failure(Cause::Interrupt));
}

#[test]
fn test_stack_context_projection_zero_heap() {
    // F03: Stack projection adapter Env_ABC -> Env_AC without heap allocation
    let service_a: u64 = 101;
    let service_b: u64 = 202;
    let service_c: u64 = 303;

    let tag_a = 1;
    let tag_b = 2;
    let tag_c = 3;

    // Full environment Env_ABC (size 3)
    let env_abc = StackContextProjection::new(
        [
            &service_a as *const u64 as *const core::ffi::c_void,
            &service_b as *const u64 as *const core::ffi::c_void,
            &service_c as *const u64 as *const core::ffi::c_void,
        ],
        [tag_a, tag_b, tag_c],
    );

    // Project down to Env_AC (size 2) on the stack
    let ptr_a = env_abc.get(tag_a).expect("service A found");
    let ptr_c = env_abc.get(tag_c).expect("service C found");

    let env_ac = StackContextProjection::new([ptr_a, ptr_c], [tag_a, tag_c]);

    assert!(env_ac.get(tag_b).is_none());
    assert_eq!(env_ac.get(tag_a), Some(&service_a as *const u64 as *const core::ffi::c_void));
    assert_eq!(env_ac.get(tag_c), Some(&service_c as *const u64 as *const core::ffi::c_void));
}

#[test]
fn test_layer_reverse_rollback_on_failure() {
    // F13: Layer assembly rollback on failure in LIFO order
    let events = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));

    // Case 1: Failure during assembly triggers automatic LIFO rollback on drop
    {
        let mut ledger = LayerRollbackLedger::new();
        let ev1 = Arc::clone(&events);
        ledger.register(move || {
            ev1.lock().unwrap().push("Finalize Layer 1 (Db)");
        });

        let ev2 = Arc::clone(&events);
        ledger.register(move || {
            ev2.lock().unwrap().push("Finalize Layer 2 (Cache)");
        });

        // Layer 3 fails to acquire -> ledger goes out of scope without commit!
    }

    let recorded = events.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec!["Finalize Layer 2 (Cache)", "Finalize Layer 1 (Db)"],
        "Rollback must finalize in strict reverse (LIFO) order"
    );

    // Case 2: Successful composition calls commit(), disarming rollback
    events.lock().unwrap().clear();
    {
        let mut ledger = LayerRollbackLedger::new();
        let ev = Arc::clone(&events);
        ledger.register(move || {
            ev.lock().unwrap().push("Should not run");
        });
        ledger.commit();
    }
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_std_effect_runtime_primitives() {
    let script = r#"
        use std.effect

        enum Exit
            Success(int)
            Fail(string)
            Die(string)
            Interrupt
        end

        function test_succeed(): Exit
            local eff = effect.succeed(42)
            return effect.run_sync(eff)
        end

        function test_fail(): Exit
            local eff = effect.fail("test error")
            return effect.run_sync(eff)
        end

        function test_die(): Exit
            local eff = effect.die("fatal defect")
            return effect.run_sync(eff)
        end

        function test_interrupt(): Exit
            local eff = effect.interrupt()
            return effect.run_sync(eff)
        end
    "#;

    let mut prog = EmbeddedProgram::builder()
        .module("main", script)
        .entry_module("main")
        .compile()
        .expect("compile");

    let res1 = prog.vm_mut().call("main.test_succeed", vec![]).expect("call succeed");
    if let Value::Enum(e) = res1 {
        assert_eq!(e.enum_name, "Exit");
        assert_eq!(e.variant, "Success");
        assert_eq!(e.values.as_ref().unwrap()[0], Value::Int(42));
    } else {
        panic!("expected Exit enum for res1, got {:?}", res1);
    }

    let res2 = prog.vm_mut().call("main.test_fail", vec![]).expect("call fail");
    if let Value::Enum(e) = res2 {
        assert_eq!(e.enum_name, "Exit");
        assert_eq!(e.variant, "Fail");
        assert_eq!(e.values.as_ref().unwrap()[0], Value::string("test error"));
    } else {
        panic!("expected Exit enum for res2, got {:?}", res2);
    }

    let res3 = prog.vm_mut().call("main.test_die", vec![]).expect("call die");
    if let Value::Enum(e) = res3 {
        assert_eq!(e.enum_name, "Exit");
        assert_eq!(e.variant, "Die");
        assert_eq!(e.values.as_ref().unwrap()[0], Value::string("fatal defect"));
    } else {
        panic!("expected Exit enum for res3, got {:?}", res3);
    }

    let res4 = prog.vm_mut().call("main.test_interrupt", vec![]).expect("call interrupt");
    if let Value::Enum(e) = res4 {
        assert_eq!(e.enum_name, "Exit");
        assert_eq!(e.variant, "Interrupt");
    } else {
        panic!("expected Exit enum for res4, got {:?}", res4);
    }
}

#[test]
fn test_gate_neg_03_replay_blueprint_purity() {
    // Gate-NEG-03: Effect is a cold blueprint; multiple run_sync executions run side-effects independently
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let script = r#"
        use std.effect

        extern
            function increment_host_counter(): int
        end

        enum Exit
            Success(int)
            Fail(string)
            Die(string)
            Interrupt
        end

        function run_blueprint_twice(): Exit
            local blueprint = effect.suspend(function(): int
                return increment_host_counter()
            end)

            effect.run_sync(blueprint)
            return effect.run_sync(blueprint)
        end
    "#;

    let mut prog = EmbeddedProgram::builder()
        .module("main", script)
        .entry_module("main")
        .compile()
        .expect("compile");

    prog.register_native_fn("main.increment_host_counter", move |_args: &[Value]| {
        let val = COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(lust::bytecode::NativeCallResult::Return(Value::Int(val as i64)))
    });

    assert_eq!(COUNTER.load(Ordering::SeqCst), 0);

    let ret = prog.vm_mut().call("main.run_blueprint_twice", vec![]).expect("call blueprint test");
    if let Value::Enum(e) = ret {
        assert_eq!(e.variant, "Success");
    }
    assert_eq!(COUNTER.load(Ordering::SeqCst), 2);
}
