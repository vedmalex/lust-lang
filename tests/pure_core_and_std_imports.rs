use lust::{
    EmbeddedBuilder, EmbeddedModule, LustConfig,
    bytecode::Value,
    load_program_from_embedded_with_config,
    compile_program_with_config,
};

#[test]
fn test_pure_mode_rejects_ambient_globals() {
    // 1. Ambient print function should be rejected in Pure mode with helpful import hint
    let source_print = r#"
        function run(): int
            print(42)
            return 0
        end
    "#;
    let entries_print = &[EmbeddedModule {
        module: "main",
        parent: None,
        source: Some(source_print),
    }];
    let config = LustConfig::pure();
    let program = load_program_from_embedded_with_config(entries_print, "main", &config)
        .expect("load program");
    let err = match compile_program_with_config(program, &config) {
        Err(e) => e,
        Ok(_) => panic!("print() must fail in pure mode without explicit import"),
    };
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("print"),
        "error should mention print: {}",
        err_str
    );
    assert!(
        err_str.contains("use std.io.{print};"),
        "error should contain import hint: {}",
        err_str
    );

    // 2. Ambient math module should be rejected in Pure mode with helpful import hint
    let source_math = r#"
        function run(): int
            local x: int = math.abs(-5)
            return x
        end
    "#;
    let entries_math = &[EmbeddedModule {
        module: "main",
        parent: None,
        source: Some(source_math),
    }];
    let program_math = load_program_from_embedded_with_config(entries_math, "main", &config)
        .expect("load program");
    let err_math = match compile_program_with_config(program_math, &config) {
        Err(e) => e,
        Ok(_) => panic!("math.abs() must fail in pure mode without explicit import"),
    };
    let err_math_str = format!("{:?}", err_math);
    assert!(
        err_math_str.contains("math") || err_math_str.contains("abs"),
        "error should mention math or abs: {}",
        err_math_str
    );
    assert!(
        err_math_str.contains("use std.math"),
        "error should contain import hint: {}",
        err_math_str
    );

    // 3. Confirm that the exact same scripts succeed in Standard mode (backward compatibility)
    let standard_config = LustConfig::default();
    let program_std = load_program_from_embedded_with_config(entries_math, "main", &standard_config)
        .expect("load program in standard mode");
    let mut vm_std = compile_program_with_config(program_std, &standard_config)
        .expect("compilation in standard mode must succeed");
    let res = vm_std.call("run", vec![]).expect("call in standard mode");
    assert_eq!(res, Value::Int(5));
}

#[test]
fn test_pure_mode_with_explicit_imports() {
    let source = r#"
        use std.math.{abs, min}
        use std.table

        function compute(): int
            local a: int = abs(-5)
            local b: int = min(10, 20)
            local tbl = [1, 2, 3]
            local len = table.len(tbl)
            return a + b + len
        end
    "#;

    let entries = &[EmbeddedModule {
        module: "main",
        parent: None,
        source: Some(source),
    }];

    let config = LustConfig::pure();
    let program = load_program_from_embedded_with_config(entries, "main", &config)
        .expect("load program with explicit imports in pure mode");
    let mut vm = compile_program_with_config(program, &config)
        .expect("compile program with explicit std imports");
    let result = vm.call("compute", vec![]).expect("call compute");
    // abs(-5) = 5, min(10, 20) = 10, table.len([1,2,3]) = 3 -> 5 + 10 + 3 = 18
    assert_eq!(result, Value::Int(18));
}

#[test]
fn test_pure_mode_capability_denied() {
    let source = r#"
        use std.io.{print}

        function main(): int
            print("hello")
            return 0
        end
    "#;

    let entries = &[EmbeddedModule {
        module: "main",
        parent: None,
        source: Some(source),
    }];

    // Case A: Pure mode with strict capabilities denies non-pure std.io
    let config_denied = LustConfig::pure().with_strict_capabilities(true);
    let err = load_program_from_embedded_with_config(entries, "main", &config_denied)
        .expect_err("pure mode without io capability must deny std.io");
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("CapabilityDenied: module 'std.io' is not enabled in this sandbox")
            || err_str.contains("CapabilityDenied: module 'io' is not enabled in this sandbox"),
        "error should indicate capability denied: {}",
        err_str
    );

    // Case B: Pure mode with explicit allow_std_module("io") succeeds
    let mut config_allowed = LustConfig::pure();
    config_allowed.allow_std_module("io");
    let program = load_program_from_embedded_with_config(entries, "main", &config_allowed)
        .expect("load program with io allowed");
    let vm = compile_program_with_config(program, &config_allowed);
    assert!(vm.is_ok(), "compilation must succeed when capability is granted");
}

#[test]
fn test_cache_isolation_between_modes() {
    let source_ambient = r#"
        function run(): int
            print(42)
            return 42
        end
    "#;

    // In Standard mode, print is ambient -> build succeeds and populates cache
    let builder_std = EmbeddedBuilder::new()
        .module("main", source_ambient)
        .entry_module("main");
    assert!(
        builder_std.compile().is_ok(),
        "standard mode should compile ambient print"
    );

    // In Pure mode, identical source must NOT hit standard cache and must fail
    let builder_pure = EmbeddedBuilder::pure()
        .module("main", source_ambient)
        .entry_module("main");
    let pure_res = builder_pure.compile();
    assert!(
        pure_res.is_err(),
        "pure mode must not reuse standard cached bytecode with ambient print"
    );

    // Pure mode with clean code and explicit imports succeeds and runs
    let source_clean = r#"
        use std.math.{abs}
        function run(): int
            return abs(-100)
        end
    "#;
    let mut prog = EmbeddedBuilder::pure()
        .module("main", source_clean)
        .entry_module("main")
        .compile()
        .expect("build clean pure script");
    let res = prog.call_raw("main.run", vec![]).expect("run clean pure script");
    assert_eq!(res, Value::Int(100));
}
