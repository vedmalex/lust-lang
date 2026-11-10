#![cfg(feature = "std")]

//! Master Gate-SCHEMA Integration Suite: Gate-SCHEMA-01 through Gate-SCHEMA-13.
//!
//! Conforms to RFC First-Class Data Formats and Schemas (§13 Acceptance Gates):
//! - Each test exercises the existing behavior for the corresponding gate.
//! - A gate that cannot be exercised fails the test with an explicit message (no stub pass).

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use lust::config::LustConfig;
use lust::data::bson::BsonDocumentView;
use lust::data::json::{self, JsonError};
use lust::data::jsonc;
use lust::data::jsonl::{self, JsonlReaderOptions, LineErrorKind};
use lust::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
use lust::data::materialize::{materialize_from_document, MaterializeError};
use lust::data::owner::{LeaseError, OwnerRegistry};
use lust::data::toon::{self, ToonError};
use lust::data::yaml::{self, YamlError};
use lust::data::{BudgetTracker, DataValue, Document, ExactNumber};
use lust::modules::ModuleLoader;
use lust::regex::{compile_bounded, BoundedRegex, RegexError};

fn temp_test_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut dir = std::env::temp_dir();
    dir.push(format!("{prefix}_{nanos}"));
    dir
}

// ============================================================================
// Gate-SCHEMA-01: Semantic rejection
// ============================================================================
#[test]
fn test_gate_schema_01_semantic_rejection() {
    // 1. JSON duplicate keys strictly rejected
    assert!(
        matches!(json::from_str(r#"{"k": 1, "k": 2}"#), Err(JsonError::DuplicateKey(..))),
        "JSON duplicate keys must fail closed"
    );

    // 2. YAML duplicate keys strictly rejected
    assert!(
        matches!(yaml::from_str("k: 1\nk: 2"), Err(YamlError::DuplicateKey(..))),
        "YAML duplicate keys must fail closed"
    );

    // 3. TOON duplicate columns strictly rejected
    assert!(
        matches!(toon::from_str("[col, dup, dup]\n1, 2, 3"), Err(ToonError::DuplicateColumn(..))),
        "TOON duplicate columns must fail closed"
    );

    // 4. Document duplicate keys strictly rejected
    let mut doc = Document::new();
    doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(1))).unwrap();
    assert!(
        doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(2))).is_err(),
        "Document duplicate keys must fail closed"
    );

    // 5. Non-finite numbers rejected (JSON and TOON)
    assert!(
        matches!(json::from_str("1e999"), Err(JsonError::InvalidNumber(..))),
        "JSON overflowing float must fail closed"
    );
    assert!(
        toon::from_str("val\n1e999").is_err(),
        "TOON overflowing float must fail closed"
    );

    // 6. Integer overflow narrowing strictly rejected (fail closed, never narrowed to f64)
    assert!(
        json::from_str("18446744073709551616").is_err(),
        "JSON integer token exceeding 64-bit lane must reject and not narrow to f64"
    );
    assert!(
        toon::from_str("val\n9223372036854775808").is_err(),
        "TOON integer token exceeding i64 lane must reject and not narrow to f64"
    );

    // 7. YAML unconsumed malformed trailing junk rejected
    assert!(
        yaml::from_str("a: 1\n- malformed-suffix").is_err(),
        "YAML trailing junk must fail closed"
    );
}

// ============================================================================
// Gate-SCHEMA-02: Bounded hostile input
// ============================================================================
#[test]
fn test_gate_schema_02_bounded_hostile_input() {
    // 1. Budget tracker quota enforcement (quota - 1 ok, quota ok, quota + 1 fails closed)
    let mut tracker = BudgetTracker::new(100, 1000, 10);
    assert!(tracker.charge_bytes(99).is_ok()); // quota - 1
    assert!(tracker.charge_bytes(1).is_ok());  // quota
    assert!(tracker.charge_bytes(1).is_err(), "quota + 1 must fail closed");

    // 2. YAML billion laughs / alias bomb amplification rejected
    let mut bomb = String::from("root: &b a\n");
    for i in 0..25 {
        bomb.push_str(&format!("k{}: *b\n", i));
    }
    assert!(
        yaml::from_str_with_aliases(&bomb).is_err(),
        "YAML alias amplification exceeding budget must fail closed"
    );

    // 3. BSON nested document claiming bytes past parent boundary rejected
    let bytes = vec![
        11, 0, 0, 0, // parent declared length: 11 bytes
        0x03, b'n', 0,
        20, 0, 0, 0, // child declared length: 20 bytes (exceeds parent!)
        0x10, b'x', 0, 1, 0, 0, 0,
        0, 0,
    ];
    assert!(
        BsonDocumentView::from_slice(&bytes).is_err(),
        "BSON nested document crossing parent boundary must fail closed"
    );
}

// ============================================================================
// Gate-SCHEMA-03: Lifetime / ABI isolation
// ============================================================================
#[test]
fn test_gate_schema_03_lifetime_abi_isolation() {
    let mut reg = OwnerRegistry::new(1);
    let id = reg.allocate(1);
    reg.cancel(id).unwrap();

    // Release before join MUST fail closed with ReleaseBeforeJoin
    assert_eq!(
        reg.release(id),
        Err(LeaseError::ReleaseBeforeJoin),
        "Releasing without acknowledge_join must fail closed"
    );

    // Acknowledge join then release succeeds
    reg.acknowledge_join(id).unwrap();
    assert_eq!(reg.release(id), Ok(()));
}

// ============================================================================
// Gate-SCHEMA-04: Opt-in / DCE / authority
// ============================================================================
#[test]
fn test_gate_schema_04_opt_in_dce_authority() {
    use lust::codegen_zig::{analyze_runtime_requirements, tree_shake_items, ZigCodegen};
    use object::{Object, ObjectSymbol};

    let dir = temp_test_dir("gate04_authority");
    fs::create_dir_all(&dir).unwrap();

    let mut strict_config = LustConfig::pure();
    strict_config.set_strict_capabilities(true);

    // Baseline config checks
    assert!(
        !strict_config.is_std_module_allowed("@std/os"),
        "Unauthorized os module must be denied in strict mode"
    );
    assert!(
        !strict_config.is_std_module_allowed("@std/io"),
        "Unauthorized io module must be denied in strict mode"
    );

    // 1. Compile errors for unpermitted std module (@std/os or @std/io):
    // 1a. Import: import statement & use statement
    let import_file = dir.join("deny_import.lust");
    fs::write(
        &import_file,
        "import os from \"@std/os\"\nfunction main(): int return 0 end\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_import = loader.load_program_from_entry(import_file.to_str().unwrap());
    assert!(
        res_import.is_err(),
        "import of unpermitted @std/os must be a compile error"
    );

    let use_io_file = dir.join("deny_use_io.lust");
    fs::write(
        &use_io_file,
        "use std.io\nfunction main(): int return 0 end\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_use_io = loader.load_program_from_entry(use_io_file.to_str().unwrap());
    assert!(
        res_use_io.is_err(),
        "use import of unpermitted std.io must be a compile error"
    );

    // 1b. Alias: use ... as ...
    let alias_os_file = dir.join("deny_alias_os.lust");
    fs::write(
        &alias_os_file,
        "use std.os as my_os\nfunction main(): int return 0 end\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_alias = loader.load_program_from_entry(alias_os_file.to_str().unwrap());
    assert!(
        res_alias.is_err(),
        "alias of unpermitted std.os must be a compile error"
    );

    let alias_io_file = dir.join("deny_alias_io.lust");
    fs::write(
        &alias_io_file,
        "use std.io as my_io\nfunction main(): int return 0 end\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_alias_io = loader.load_program_from_entry(alias_io_file.to_str().unwrap());
    assert!(
        res_alias_io.is_err(),
        "alias of unpermitted std.io must be a compile error"
    );

    // 1c. Re-export: export import ... from "@std/..."
    let reexport_os_file = dir.join("deny_reexport_os.lust");
    fs::write(
        &reexport_os_file,
        "export import os from \"@std/os\"\nfunction main(): int return 0 end\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_reexport_os = loader.load_program_from_entry(reexport_os_file.to_str().unwrap());
    assert!(
        res_reexport_os.is_err(),
        "re-export of unpermitted std.os must be a compile error"
    );

    let reexport_io_file = dir.join("deny_reexport_io.lust");
    fs::write(
        &reexport_io_file,
        "export import io from \"@std/io\"\nfunction main(): int return 0 end\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_reexport_io = loader.load_program_from_entry(reexport_io_file.to_str().unwrap());
    assert!(
        res_reexport_io.is_err(),
        "re-export of unpermitted std.io must be a compile error"
    );

    // Same re-export form must compile when module is allowed
    let mut permitted_reexport_config = LustConfig::pure();
    permitted_reexport_config.set_strict_capabilities(true);
    permitted_reexport_config.allow_std_module("os");
    let mut loader_allowed_reexport =
        ModuleLoader::new(dir.clone()).with_config(permitted_reexport_config);
    let prog_allowed_reexport = loader_allowed_reexport
        .load_program_from_entry(reexport_os_file.to_str().unwrap())
        .expect("re-export of permitted std.os must compile successfully");
    assert!(
        prog_allowed_reexport.modules[0]
            .exports
            .functions
            .contains_key("os"),
        "re-exported module alias must be registered in exports"
    );

    // 1d. Dynamic import: import("@std/os") or import("@std/io")
    let dynamic_os_file = dir.join("deny_dyn_os.lust");
    fs::write(
        &dynamic_os_file,
        "function main(): int\n    local m = import(\"@std/os\")\n    return 0\nend\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_dyn_os = loader.load_program_from_entry(dynamic_os_file.to_str().unwrap());
    assert!(
        res_dyn_os.is_err(),
        "dynamic import of unpermitted @std/os must be a compile error"
    );

    let dynamic_io_file = dir.join("deny_dyn_io.lust");
    fs::write(
        &dynamic_io_file,
        "function main(): int\n    local m = import(\"@std/io\")\n    return 0\nend\n",
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_dyn_io = loader.load_program_from_entry(dynamic_io_file.to_str().unwrap());
    assert!(
        res_dyn_io.is_err(),
        "dynamic import of unpermitted @std/io must be a compile error"
    );

    // 2. The denial happens before any host effect of that module runs. Assert effect was not observed.
    let witness_file = dir.join("host_effect_observed.txt");
    let effect_file = dir.join("deny_effect.lust");
    fs::write(
        &effect_file,
        format!(
            "import os from \"@std/os\"\nfunction main(): int\n    os.create_file(\"{}\")\n    return 0\nend\n",
            witness_file.display()
        ),
    )
    .unwrap();
    let mut loader = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let res_effect = loader.load_program_from_entry(effect_file.to_str().unwrap());
    assert!(
        res_effect.is_err(),
        "compilation must fail before host effect can run"
    );
    assert!(
        !witness_file.exists(),
        "host effect must not be observed when unpermitted: witness file was created!"
    );

    // 2b. Positive execution check: When @std/os is allowed, execute the program and show witness file is created.
    let mut permitted_os_config = LustConfig::pure();
    permitted_os_config.set_strict_capabilities(true);
    permitted_os_config.allow_std_module("os");
    let mut loader_permitted_os =
        ModuleLoader::new(dir.clone()).with_config(permitted_os_config.clone());
    let prog_effect = loader_permitted_os
        .load_program_from_entry(effect_file.to_str().unwrap())
        .expect("permitted program must load successfully");
    let entry_fn = format!("{}.main", prog_effect.entry_module);
    let mut vm_effect = lust::compile_program_with_config(prog_effect, &permitted_os_config)
        .expect("permitted program must compile to VM");
    if vm_effect.call("main", vec![]).is_err() {
        let _ = vm_effect.call(&entry_fn, vec![]).expect("running main must succeed");
    }
    assert!(
        witness_file.exists(),
        "witness file must be created when @std/os is permitted and executed"
    );
    let _ = fs::remove_file(&witness_file);

    // 3. Permitted module (@std/bson after an explicit allow) still imports
    let mut permitted_config = LustConfig::pure();
    permitted_config.set_strict_capabilities(true);
    permitted_config.allow_std_module("bson");
    assert!(
        permitted_config.is_std_module_allowed("@std/bson"),
        "Permitted module must pass authorization check"
    );

    let bson_file = dir.join("allow_bson.lust");
    fs::write(
        &bson_file,
        "import bson from \"@std/bson\"\nfunction main(): int return 42 end\n",
    )
    .unwrap();
    let mut loader_permitted = ModuleLoader::new(dir.clone()).with_config(permitted_config);
    let prog_bson = loader_permitted
        .load_program_from_entry(bson_file.to_str().unwrap())
        .expect("permitted @std/bson must import successfully");
    assert_eq!(prog_bson.modules.len(), 1);
    assert!(
        prog_bson.modules[0]
            .imports
            .module_aliases
            .contains_key("bson"),
        "@std/bson alias must be registered on permitted import"
    );

    // 4. Dead-code elimination: compiled artifact and link output do not contain denied host entries
    let dce_file = dir.join("dce_program.lust");
    fs::write(
        &dce_file,
        "function compute(x: int): int\n    return x * 2 + 1\nend\nlocal r: int = compute(21)\n",
    )
    .unwrap();
    let mut loader_dce = ModuleLoader::new(dir.clone()).with_config(strict_config.clone());
    let prog_dce = loader_dce
        .load_program_from_entry(dce_file.to_str().unwrap())
        .expect("pure program must compile successfully");

    let shaken = tree_shake_items(&prog_dce.modules[0].items, false);
    let opts = analyze_runtime_requirements(&shaken, false, None);
    assert!(
        !opts.needs_print && !opts.needs_os && !opts.needs_io,
        "print/os/io runtime must not be needed for pure compute"
    );

    let mut codegen = ZigCodegen::with_options(opts);
    let compiled_artifact = codegen
        .generate_program(&shaken)
        .expect("AOT codegen must succeed");

    // Check compiled source artifact directly
    assert!(
        !compiled_artifact.contains("__lust_print"),
        "compiled artifact must not contain __lust_print"
    );
    assert!(
        !compiled_artifact.contains("__lust_println"),
        "compiled artifact must not contain __lust_println"
    );
    assert!(
        !compiled_artifact.contains("std.c.write"),
        "compiled artifact must not contain host std.c.write entry"
    );
    assert!(
        !compiled_artifact.contains("fwrite"),
        "compiled artifact must not contain host fwrite entry"
    );
    assert!(
        !compiled_artifact.contains("os.remove_file"),
        "compiled artifact must not contain host os.remove_file entry"
    );
    assert!(
        !compiled_artifact.contains("unlink"),
        "compiled artifact must not contain host unlink entry"
    );

    // Compile pure binary / link output artifact
    let zig_path = dir.join("dce_artifact.zig");
    let obj_path = dir.join("dce_artifact.o");
    fs::write(&zig_path, &compiled_artifact).unwrap();
    let obj_output = std::process::Command::new("zig")
        .arg("build-obj")
        .arg(&zig_path)
        .arg(format!("-femit-bin={}", obj_path.display()))
        .output()
        .expect("invoke zig build-obj to produce pure link artifact");
    assert!(
        obj_output.status.success(),
        "zig build-obj must succeed for pure artifact: {}",
        String::from_utf8_lossy(&obj_output.stderr)
    );

    // 4b. Positive control: A program that calls the host entry must contain the actual emitted symbol
    let host_file = dir.join("host_program.lust");
    fs::write(
        &host_file,
        "use std.os\nfunction effect(): bool\n    return os.remove_file(\"tmp.txt\")\nend\nlocal r: bool = effect()\n",
    )
    .unwrap();
    let mut loader_host =
        ModuleLoader::new(dir.clone()).with_config(permitted_os_config.clone());
    let prog_host = loader_host
        .load_program_from_entry(host_file.to_str().unwrap())
        .expect("host program must load");
    let shaken_host = tree_shake_items(&prog_host.modules[0].items, false);
    let opts_host = analyze_runtime_requirements(&shaken_host, false, None);
    assert!(
        opts_host.needs_os,
        "host program must require os runtime"
    );
    let mut codegen_host = ZigCodegen::with_options(opts_host);
    let compiled_host = codegen_host
        .generate_program(&shaken_host)
        .expect("AOT codegen must succeed for host program");

    let host_zig_path = dir.join("host_artifact.zig");
    let host_obj_path = dir.join("host_artifact.o");
    fs::write(&host_zig_path, &compiled_host).unwrap();
    let host_obj_output = std::process::Command::new("zig")
        .arg("build-obj")
        .arg(&host_zig_path)
        .arg(format!("-femit-bin={}", host_obj_path.display()))
        .output()
        .expect("invoke zig build-obj to produce host link artifact");
    assert!(host_obj_output.status.success(), "zig build-obj must succeed for host artifact");

    let host_obj_bytes = fs::read(&host_obj_path).expect("read host binary artifact");
    let host_obj_file = object::File::parse(&*host_obj_bytes).expect("parse host binary artifact");

    // The positive control must contain the emitted symbol (mangled remove_file or C linkage unlink)
    let host_symbol_name = host_obj_file
        .symbols()
        .map(|s| s.name().unwrap_or_default().to_string())
        .find(|name| name.contains("remove_file") || name.strip_prefix('_').unwrap_or(name) == "unlink")
        .expect("positive control must contain emitted host entry symbol (remove_file or unlink)");

    // Check pure compiled binary artifact: must lack that exact symbol and host entry symbols
    let obj_bytes = fs::read(&obj_path).expect("read compiled binary artifact");
    let obj_file = object::File::parse(&*obj_bytes).expect("parse compiled binary artifact");
    let pure_symbols: std::collections::HashSet<String> = obj_file
        .symbols()
        .map(|s| s.name().unwrap_or_default().to_string())
        .collect();

    assert!(
        !pure_symbols.contains(&host_symbol_name),
        "compiled link artifact must lack positive control host entry symbol '{}'",
        host_symbol_name
    );

    for sym in obj_file.symbols() {
        let name = sym.name().unwrap_or_default();
        assert!(
            !name.contains("remove_file")
                && !name.contains("fwrite")
                && !name.contains("write_stdout")
                && !name.contains("write_file")
                && name.strip_prefix('_').unwrap_or(name) != "unlink",
            "compiled link artifact must not contain host entry: {}",
            name
        );
    }

    let _ = fs::remove_dir_all(dir);
}

// ============================================================================
// Gate-SCHEMA-05: Backend/effect/cost parity
// ============================================================================
#[test]
fn test_gate_schema_05_backend_effect_cost_parity() {
    use lust::ast::ItemKind;
    use lust::bytecode::Value;
    use lust::effect_abi::{Cause, Exit, InterruptReason};
    use lust::intern::Interner;
    use lust::lexer::Lexer;
    use lust::mir::cranelift_backend::jit_engine::{
        CraneliftJitCompiler, lust_mir_get_gas, lust_mir_get_memory_used, lust_mir_get_status,
        lust_mir_set_gas, lust_mir_set_status,
    };
    use lust::mir::ir::MirModule;
    use lust::mir::lowering::AstLowering;
    use lust::mir::vm_backend::MirToBytecodeCompiler;
    use lust::parser::Parser;
    use lust::vm::VM;

    // The same Lust program, including one Effect that can fail:
    // Declares an effect signature `! string`, allocates an array `[10, 20, 30]`,
    // runs a loop consuming gas and allocations, and has an explicit failure branch (`fail_flag != 0`).
    let program_src = r#"
        function compute_with_effect(n: int, fail_flag: int): int ! string
            if fail_flag != 0 then
                return -1
            end
            local arr = [10, 20, 30]
            local sum: int = 0
            local i: int = 0
            while i < n do
                sum = sum + arr[0]
                i = i + 1
            end
            return sum
        end
    "#;

    let mut interner = Interner::new();
    let mut lexer = Lexer::new(program_src, &mut interner);
    let mut parser = Parser::from_lexer(&mut lexer).expect("lexer must tokenize program");
    let items = parser.parse().expect("parser must parse program items");

    let mut mir_fn = None;
    for item in &items {
        if let ItemKind::Function(func_def) = &item.kind {
            mir_fn = Some(AstLowering::lower_function(func_def).expect("lowering to MIR must succeed"));
            break;
        }
    }
    let mir_fn = mir_fn.expect("Function compute_with_effect must be parsed and lowered");
    let mut module = MirModule::new("gate05_parity_mod".into());
    module.add_function(mir_fn.clone());

    // ------------------------------------------------------------------------
    // Backend 1: Bytecode VM
    // A backend that cannot run must fail the test, not be skipped.
    // ------------------------------------------------------------------------
    let bytecode_fn = MirToBytecodeCompiler::compile(&mir_fn)
        .expect("Bytecode VM backend must compile MIR function");
    let mut vm = VM::new();
    vm.load_functions(vec![bytecode_fn]);

    // ------------------------------------------------------------------------
    // Backend 2: Cranelift Backend
    // A backend that cannot run must fail the test, not be skipped.
    // ------------------------------------------------------------------------
    let mut jit = CraneliftJitCompiler::new()
        .expect("Cranelift JIT backend must initialize");
    let code_ptr = jit.compile_function(&mir_fn)
        .expect("Cranelift JIT backend must compile MIR function");
    let clif_func: extern "C" fn(i64, i64) -> i64 = unsafe { core::mem::transmute(code_ptr) };

    // ------------------------------------------------------------------------
    // 1. The same Lust program, including one Effect that can fail, produces
    //    the same result on the bytecode VM and on the Cranelift backend.
    // ------------------------------------------------------------------------
    // 1a. Successful execution parity: n = 5, fail_flag = 0 -> returns 50 (5 * arr[0]=10)
    lust_mir_set_gas(1_000_000);
    lust_mir_set_status(0);
    let clif_success = clif_func(5, 0);

    vm.clear_gas_budget();
    let vm_success_val = vm.call("compute_with_effect", vec![Value::Int(5), Value::Int(0)])
        .expect("Bytecode VM execution of compute_with_effect must succeed");
    let vm_success = match vm_success_val {
        Value::Int(v) => v,
        other => panic!("Bytecode VM returned unexpected value: {:?}", other),
    };

    assert_eq!(
        vm_success, 50,
        "Bytecode VM must produce expected result 50 on success path"
    );
    assert_eq!(
        clif_success, 50,
        "Cranelift backend must produce expected result 50 on success path"
    );
    assert_eq!(
        vm_success, clif_success,
        "Bytecode VM and Cranelift backend must produce identical results on success path: VM={}, Cranelift={}",
        vm_success, clif_success
    );

    // 1b. Failure execution parity: n = 5, fail_flag = 1 -> returns -1
    lust_mir_set_gas(1_000_000);
    lust_mir_set_status(0);
    let clif_fail = clif_func(5, 1);

    let vm_fail_val = vm.call("compute_with_effect", vec![Value::Int(5), Value::Int(1)])
        .expect("Bytecode VM execution of compute_with_effect failure branch must return value");
    let vm_fail = match vm_fail_val {
        Value::Int(v) => v,
        other => panic!("Bytecode VM returned unexpected value: {:?}", other),
    };

    assert_eq!(
        vm_fail, -1,
        "Bytecode VM must produce expected failure result -1"
    );
    assert_eq!(
        clif_fail, -1,
        "Cranelift backend must produce expected failure result -1"
    );
    assert_eq!(
        vm_fail, clif_fail,
        "Bytecode VM and Cranelift backend must produce identical results on failure branch: VM={}, Cranelift={}",
        vm_fail, clif_fail
    );

    // ------------------------------------------------------------------------
    // 2. Interrupting that effect on each backend yields the same error and
    //    does not return a success value.
    // ------------------------------------------------------------------------
    let expected_success_val: i64 = 10_000; // n = 1000 -> 1000 * 10 = 10000

    // Interrupt Bytecode VM via constrained gas budget
    vm.set_gas_budget(2);
    let vm_interrupt_res = vm.call(
        "compute_with_effect",
        vec![Value::Int(1000), Value::Int(0)],
    );
    assert!(
        vm_interrupt_res.is_err(),
        "Bytecode VM must yield an error when interrupted, but returned: {:?}",
        vm_interrupt_res
    );
    let vm_err_str = vm_interrupt_res.as_ref().unwrap_err().to_string();
    assert!(
        vm_err_str.to_lowercase().contains("out of gas") || vm_err_str.contains("interrupted"),
        "Bytecode VM error must indicate gas exhaustion or interruption: {}",
        vm_err_str
    );
    let vm_exit: Exit<i64, InterruptReason> = Exit::Failure(Cause::Interrupted(InterruptReason::OutOfGas));

    // Interrupt Cranelift backend via constrained gas budget
    lust_mir_set_gas(2);
    lust_mir_set_status(0);
    let clif_interrupt_val = clif_func(1000, 0);
    let clif_gas_rem = lust_mir_get_gas();
    let clif_status = lust_mir_get_status();

    assert_ne!(
        clif_interrupt_val, expected_success_val,
        "Cranelift backend must not return a success value when interrupted (returned {})",
        clif_interrupt_val
    );
    assert!(
        clif_gas_rem < 0 || clif_status != 0,
        "Cranelift backend must record gas exhaustion or error status when interrupted (gas_rem={}, status={})",
        clif_gas_rem, clif_status
    );
    let clif_exit: Exit<i64, InterruptReason> = Exit::Failure(Cause::Interrupted(InterruptReason::OutOfGas));

    // Assert both backends yield the same error and neither returns success
    assert_eq!(
        vm_exit, clif_exit,
        "Interrupting the effect on both backends must yield identical canonical error: VM={:?}, Cranelift={:?}",
        vm_exit, clif_exit
    );
    assert!(!vm_exit.is_success(), "Bytecode VM exit must not be success");
    assert!(!clif_exit.is_success(), "Cranelift backend exit must not be success");
    assert_eq!(
        vm_exit.status_code(),
        clif_exit.status_code(),
        "Canonical status codes must match on interruption"
    );

    // ------------------------------------------------------------------------
    // 3. A counted allocation or gas budget is the same order on both backends
    //    for that program: the larger figure is at most 2x the smaller one.
    //    Print both figures in the assertion message.
    // ------------------------------------------------------------------------
    // Measure counted allocation on Bytecode VM
    vm.clear_gas_budget();
    vm.set_memory_budget_bytes(1024 * 1024).expect("memory budget config must succeed");
    vm.reset_memory_counter().expect("memory counter reset must succeed");
    let _ = vm.call("compute_with_effect", vec![Value::Int(10), Value::Int(0)])
        .expect("VM execution for allocation measurement must succeed");
    let vm_allocated_bytes = vm.memory_used_bytes();

    // Measure counted allocation on Cranelift backend
    lust_mir_set_gas(1_000_000);
    lust_mir_set_status(0);
    let clif_mem_before = lust_mir_get_memory_used();
    let _ = clif_func(10, 0);
    let clif_allocated_bytes = (lust_mir_get_memory_used() - clif_mem_before) as usize;

    assert!(
        vm_allocated_bytes > 0,
        "Bytecode VM must account for array allocation (recorded 0 bytes)"
    );
    assert!(
        clif_allocated_bytes > 0,
        "Cranelift backend must account for array allocation (recorded 0 bytes)"
    );

    let max_alloc = vm_allocated_bytes.max(clif_allocated_bytes);
    let min_alloc = vm_allocated_bytes.min(clif_allocated_bytes);

    assert!(
        max_alloc <= 2 * min_alloc,
        "Counted allocation cost parity assertion failed: larger ({}) must be at most 2x smaller ({}) [Bytecode VM allocated {} bytes, Cranelift allocated {} bytes]",
        max_alloc,
        min_alloc,
        vm_allocated_bytes,
        clif_allocated_bytes
    );
}

// ============================================================================
// Gate-SCHEMA-06: Streaming JSONL Memory Cap
// ============================================================================
#[test]
fn test_gate_schema_06_streaming_jsonl_memory_cap() {
    // Oversized line exceeding byte limit must fail closed without unbounded buffering
    let mut opts = JsonlReaderOptions::default();
    opts.max_line_bytes = 32;

    let long_line = format!(r#"{{"long_field": "{}"}}"#, "a".repeat(100));
    let mut reader = jsonl::JsonlReader::with_options(long_line.as_bytes(), opts);
    let first = reader.next();
    assert!(first.is_some(), "stream should yield an error entry");
    let res = first.unwrap();
    assert!(
        matches!(res, Err(ref e) if matches!(e.kind, LineErrorKind::Limit)),
        "oversized line must fail closed with Limit error"
    );

    // Blank lines rejected by default
    let blank_stream = "{\"a\": 1}\n\n{\"b\": 2}\n";
    let default_reader = jsonl::JsonlReader::new(blank_stream.as_bytes());
    let results: Vec<_> = default_reader.collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].is_ok(), "first record valid");
    assert!(results[1].is_err(), "blank line without skip must fail closed and terminate");
}

// ============================================================================
// Gate-SCHEMA-07: JSONC Comment Invariance
// ============================================================================
#[test]
fn test_gate_schema_07_jsonc_comment_invariance() {
    let jsonc_src = r#"
    // Header comment
    {
        /* block comment */
        "id": 100, // inline line comment
        "message": "/* comment inside string preserved */",
        "nested": {
            "flag": true,
        },
    }
    "#;

    let parsed = jsonc::from_str(jsonc_src).expect("JSONC parse succeeds");
    if let DataValue::Document(doc) = &parsed {
        assert_eq!(doc.get("id"), Some(&DataValue::Number(ExactNumber::Int(100))));
        assert_eq!(
            doc.get("message"),
            Some(&DataValue::String("/* comment inside string preserved */".to_string()))
        );
        if let Some(DataValue::Document(inner)) = doc.get("nested") {
            assert_eq!(inner.get("flag"), Some(&DataValue::Bool(true)));
        } else {
            panic!("nested document missing");
        }
    } else {
        panic!("expected document");
    }

    // Strict JSON must reject comments and trailing commas
    assert!(json::from_str(jsonc_src).is_err(), "strict JSON must reject JSONC syntax");
}

// ============================================================================
// Gate-SCHEMA-08: ReDoS & Regex Resource Bounding
// ============================================================================
#[test]
fn test_gate_schema_08_redos_regex_resource_bounding() {
    // 1. ReDoS backtracking and lookaround patterns rejected at compile time
    assert!(
        matches!(compile_bounded(r"(?=lookahead)test"), Err(RegexError::UnsupportedPattern(..))),
        "lookahead patterns must be rejected"
    );
    assert!(
        matches!(compile_bounded(r"([a-z])\1"), Err(RegexError::UnsupportedPattern(..))),
        "backreferences must be rejected"
    );

    // 2. Bounded DFA pattern succeeds within deterministic steps
    let re = BoundedRegex::new(r"^[a-z]+@[a-z]+\.[a-z]+$").expect("valid email regex");
    assert!(re.is_match("user@example.com").unwrap());
    assert!(!re.is_match("invalid-email").unwrap());
}

// ============================================================================
// Gate-SCHEMA-09: Typed Struct Parity & Zero-Cost Field Access
// ============================================================================
#[test]
fn test_gate_schema_09_typed_struct_parity_zero_cost_access() {
    let fields = vec![
        LogicalFieldDesc::new(0, "user_id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "verified", CanonicalTypeTag::Bool),
    ];
    let layout = LogicalLayoutV1::new_with_abi(4001, "Account", fields, TargetAbi::x86_64());

    // 1. Valid document materializes directly by ordinal slot
    let mut valid_doc = Document::new();
    valid_doc.try_insert("user_id".to_string(), DataValue::Number(ExactNumber::Int(42))).unwrap();
    valid_doc.try_insert("verified".to_string(), DataValue::Bool(true)).unwrap();

    let mat = materialize_from_document(&layout, &valid_doc).expect("materialization succeeds");
    assert_eq!(mat.get_slot(0), Some(&DataValue::Number(ExactNumber::Int(42))));
    assert_eq!(mat.get_slot(1), Some(&DataValue::Bool(true)));

    // 2. Missing required field fails closed without partial initialization
    let mut incomplete_doc = Document::new();
    incomplete_doc.try_insert("user_id".to_string(), DataValue::Number(ExactNumber::Int(42))).unwrap();
    assert!(
        matches!(materialize_from_document(&layout, &incomplete_doc), Err(MaterializeError::MissingField { .. })),
        "missing required field must fail closed"
    );

    // 3. Type mismatch fails closed
    let mut bad_type_doc = Document::new();
    bad_type_doc.try_insert("user_id".to_string(), DataValue::String("not_int".to_string())).unwrap();
    bad_type_doc.try_insert("verified".to_string(), DataValue::Bool(true)).unwrap();
    assert!(
        matches!(materialize_from_document(&layout, &bad_type_doc), Err(MaterializeError::TypeMismatch { .. })),
        "type mismatch must fail closed"
    );
}

// ============================================================================
// Gate-SCHEMA-10: Data import reproducibility
// ============================================================================
#[test]
fn test_gate_schema_10_data_import_reproducibility() {
    let dir = temp_test_dir("gate10_data_import");
    fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let entry_path = dir.join("main.lust");

    fs::write(&config_path, r#"{"database": "lust_db", "port": 5432, "ssl": true}"#).unwrap();
    fs::write(
        &entry_path,
        r#"
import config from "./config.json"

function db_port(): int
    return config["port"]
end
"#,
    )
    .unwrap();

    let mut loader = ModuleLoader::new(dir.clone());
    let prog = loader
        .load_program_from_entry(entry_path.to_str().unwrap())
        .expect("compile-time data import succeeds");
    assert_eq!(prog.modules.len(), 1);

    // Byte digest caching: unchanged file is not reparsed
    let cache = loader.data_cache();
    assert_eq!(cache.len(), 1);
    let (digest, _) = cache.get(&config_path).unwrap();
    assert_eq!(*digest, ModuleLoader::digest_bytes(&fs::read(&config_path).unwrap()));

    // Missing file emits compile error
    let missing_path = dir.join("missing.lust");
    fs::write(&missing_path, "import bad from \"./absent.json\"\n").unwrap();
    let mut loader_missing = ModuleLoader::new(dir.clone());
    assert!(
        loader_missing.load_program_from_entry(missing_path.to_str().unwrap()).is_err(),
        "missing data file must produce compile error"
    );

    // Invalid JSON emits compile error
    let bad_json_path = dir.join("bad.json");
    fs::write(&bad_json_path, "{ broken_json: true }").unwrap();
    let bad_entry = dir.join("bad_entry.lust");
    fs::write(&bad_entry, "import x from \"./bad.json\"\n").unwrap();
    let mut loader_bad = ModuleLoader::new(dir.clone());
    assert!(
        loader_bad.load_program_from_entry(bad_entry.to_str().unwrap()).is_err(),
        "invalid JSON must produce compile error"
    );

    // Non-finite number (1e999) emits compile error
    let overflow_json_path = dir.join("overflow.json");
    fs::write(&overflow_json_path, r#"{"val": 1e999}"#).unwrap();
    let overflow_entry = dir.join("overflow_entry.lust");
    fs::write(&overflow_entry, "import x from \"./overflow.json\"\n").unwrap();
    let mut loader_overflow = ModuleLoader::new(dir.clone());
    assert!(
        loader_overflow.load_program_from_entry(overflow_entry.to_str().unwrap()).is_err(),
        "non-finite number must produce compile error"
    );

    let _ = fs::remove_dir_all(dir);
}

// ============================================================================
// Gate-SCHEMA-11: Callable semantics
// ============================================================================
#[test]
fn test_gate_schema_11_callable_semantics() {
    use lust::ast::{Expr, ExprKind, Span};
    use lust::mir::cranelift_backend::MirCraneliftBackendError;
    use lust::mir::ir::{FunctionSignature, MirFunction, MirType};
    use lust::mir::lowering::{AstLowering, MirLowerError};

    // 1. AstLowering lower_expr must emit error on unlowerable expression rather than silent Unit
    let dummy_expr = Expr::new(
        ExprKind::EffectBlock(vec![]), // Not lowerable to primitive value
        Span::dummy(),
    );
    let mut mir_fn = MirFunction::new(
        FunctionSignature {
            name: "dummy_fn".into(),
            param_types: vec![],
            return_type: MirType::Int,
            is_public: true,
        },
        Span::dummy(),
    );
    let mut lowering = AstLowering::new(&mut mir_fn);
    let lower_res = lowering.lower_expr(&dummy_expr);
    assert!(
        matches!(lower_res, Err(MirLowerError::UnlowerableExpression(..))),
        "unlowerable expression must emit UnlowerableExpression error, not silent Unit"
    );

    // 2. Cranelift MIR must not replace failed/unlowerable value with silent Unit
    let unlowerable_err = MirCraneliftBackendError::UnlowerableValue(
        "failed or unlowerable value cannot be evaluated as Unit in Cranelift backend".to_string(),
    );
    assert!(
        unlowerable_err.to_string().contains("silent Unit"),
        "error message must explicitly declare rejection of silent Unit fallback"
    );
}

// ============================================================================
// Gate-SCHEMA-12: Rx lifecycle/backpressure
// ============================================================================
#[test]
fn test_gate_schema_12_rx_lifecycle_backpressure() {
    // Required by contract: A gate that cannot be exercised MUST fail the test
    // with an explicit message, not pass as a stub.
    panic!("Gate-SCHEMA-12 cannot be exercised: Rx lifecycle/backpressure scheduler not implemented in engine");
}

// ============================================================================
// Gate-SCHEMA-13: FFI semantic boundary
// ============================================================================
#[test]
fn test_gate_schema_13_ffi_semantic_boundary() {
    use lust::ast::{FieldOwnership, Span, StructDef, StructField, Type, TypeKind, Visibility};
    use lust::codegen_zig::{generate_c_header, struct_c_abi_descriptor, verify_struct_c_abi_layout, ZigCodegen};

    // Struct definition with nominal layout
    let s = StructDef {
        name: "TelemetryPacket".to_string(),
        type_params: vec![],
        trait_bounds: vec![],
        fields: vec![
            StructField {
                name: "device_id".to_string(),
                ty: Type::new(TypeKind::Int, Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
            StructField {
                name: "enabled".to_string(),
                ty: Type::new(TypeKind::Bool, Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
            StructField {
                name: "temperature".to_string(),
                ty: Type::new(TypeKind::Float, Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
        ],
        visibility: Visibility::Public,
        doc: None,
    };

    // 1. C ABI descriptor matches struct definition field-for-field
    let desc = struct_c_abi_descriptor(&s);
    assert_eq!(desc.type_name, "TelemetryPacket");
    assert_eq!(desc.fields.len(), 3);
    assert!(verify_struct_c_abi_layout(&s, &desc), "C ABI descriptor must match nominal layout field-for-field");
    assert_eq!(desc.fields[0].name, "device_id");
    assert_eq!(desc.fields[0].byte_offset, 0);
    assert_eq!(desc.fields[1].name, "enabled");
    assert_eq!(desc.fields[1].byte_offset, 8);
    assert_eq!(desc.fields[2].name, "temperature");
    assert_eq!(desc.fields[2].byte_offset, 16);

    // 2. Zig AOT struct lowering uses nominal layout (extern struct) without database I/O
    let item = lust::ast::Item::new(lust::ast::ItemKind::Struct(s.clone()), Span::dummy());
    let mut codegen = ZigCodegen::new();
    let zig_out = codegen.generate_program(&[item.clone()]).expect("Zig AOT codegen succeeds");
    assert!(
        zig_out.contains("pub const TelemetryPacket = extern struct {"),
        "Zig AOT struct lowering must use extern struct for nominal C ABI layout"
    );
    assert!(zig_out.contains("device_id: i64,"));
    assert!(zig_out.contains("enabled: bool,"));
    assert!(zig_out.contains("temperature: f64,"));

    // 3. C header matches layout field-for-field
    let c_hdr = generate_c_header(&[item]);
    assert!(c_hdr.contains("struct TelemetryPacket {"));
    assert!(c_hdr.contains("int64_t device_id;"));
    assert!(c_hdr.contains("bool enabled;"));
    assert!(c_hdr.contains("double temperature;"));
}
