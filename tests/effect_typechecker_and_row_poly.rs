#![cfg(feature = "std")]

use lust::{
    ast::{ItemKind, TypeKind},
    lexer::Lexer,
    parser::Parser,
    typechecker::TypeChecker,
};

fn typecheck_program(source: &str) -> lust::error::Result<Vec<lust::Item>> {
    let mut interner = lust::intern::Interner::new();
    let tokens = Lexer::new(source, &mut interner).tokenize()?;
    let items = Parser::new(tokens).parse()?;
    let mut checker = TypeChecker::new();
    checker.check_module(&items)?;
    Ok(items)
}

#[test]
fn test_defer_prohibit_return_and_break() {
    // 1. Return inside defer is illegal
    let src_bad_return = r#"
        function test_bad_return(): int
            defer return 42
            return 1
        end
    "#;
    let err = typecheck_program(src_bad_return).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("'return' is not allowed inside defer/errdefer block"),
        "expected defer return error, got: {}",
        msg
    );

    // 2. Return inside errdefer is illegal
    let src_bad_errdefer = r#"
        function test_bad_errdefer(): int
            errdefer return 42
            return 1
        end
    "#;
    let err = typecheck_program(src_bad_errdefer).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("'return' is not allowed inside defer/errdefer block"),
        "expected errdefer return error, got: {}",
        msg
    );

    // 3. Break inside defer without loop is illegal
    let src_bad_break = r#"
        function test_bad_break()
            while true do
                defer break
            end
        end
    "#;
    let err = typecheck_program(src_bad_break).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("'break' is not allowed inside defer/errdefer block"),
        "expected defer break error, got: {}",
        msg
    );

    // 4. Continue inside defer without loop is illegal
    let src_bad_continue = r#"
        function test_bad_continue()
            while true do
                defer continue
            end
        end
    "#;
    let err = typecheck_program(src_bad_continue).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("'continue' is not allowed inside defer/errdefer block"),
        "expected defer continue error, got: {}",
        msg
    );

    // 5. Break/continue inside loop that is inside defer is valid
    let src_good_loop = r#"
        function test_good_defer_loop()
            defer do
                while true do
                    break
                end
            end
            return
        end
    "#;
    assert!(typecheck_program(src_good_loop).is_ok());
}

#[test]
fn test_nominal_service_declarations_and_distinctness() {
    let src = r#"
        struct Database
            id: int
        end
        service PrimaryDb: Database
        service ReplicaDb: Database

        function sync_data(): bool ! string ~ (PrimaryDb + ReplicaDb)
            return true
        end
    "#;
    let items = typecheck_program(src).expect("nominal services typecheck failed");
    assert_eq!(items.len(), 4);

    if let ItemKind::Function(f) = &items[3].kind {
        let spec = f.return_spec();
        assert_eq!(spec.requirements.len(), 2);
        assert_eq!(
            spec.requirements[0].kind,
            TypeKind::Named("PrimaryDb".to_string())
        );
        assert_eq!(
            spec.requirements[1].kind,
            TypeKind::Named("ReplicaDb".to_string())
        );
    } else {
        panic!("expected function item");
    }
}

#[test]
fn test_effect_block_type_inference() {
    let src = r#"
        function test_effects()
            local e1 = effect do return 42 end
            local e2 = effect do
                local s = "hello"
                return s
            end
            local e3 = effect do
                local x = 10
            end
            return
        end
    "#;
    assert!(typecheck_program(src).is_ok());
}

#[test]
fn test_row_polymorphism_unordered_set_equality() {
    let src = r#"
        struct ServiceA
        end
        struct ServiceB
        end
        service A: ServiceA
        service B: ServiceB

        function take_ba(eff: Effect<int, string, B + A>): int
            return 1
        end

        function test_call(eff_ab: Effect<int, string, A + B>): int
            return take_ba(eff_ab)
        end
    "#;
    assert!(typecheck_program(src).is_ok());
}

#[test]
fn test_requirement_propagation_and_caller_validation() {
    // 1. Caller without declared requirements fails
    let src_unsatisfied = r#"
        struct Fs
        end
        service FileSystem: Fs

        function read_file(path: string): string ! string ~ FileSystem
            return path
        end

        function pure_caller(): string
            return read_file("test.txt")
        end
    "#;
    let err = typecheck_program(src_unsatisfied).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Unsatisfied requirement: 'FileSystem'"),
        "expected unsatisfied requirement error, got: {}",
        msg
    );

    // 2. Caller declaring requirements passes
    let src_satisfied = r#"
        struct Fs
        end
        struct Con
        end
        service FileSystem: Fs
        service Console: Con

        function read_file(path: string): string ! string ~ FileSystem
            return path
        end

        function safe_caller(): Result<string, string> ! string ~ (FileSystem + Console)
            return read_file("test.txt")
        end
    "#;
    assert!(typecheck_program(src_satisfied).is_ok());

    // 3. Effect block captures requirement without caller needing to declare it
    let src_captured_in_effect = r#"
        struct Fs
        end
        service FileSystem: Fs

        function read_file(path: string): string ! string ~ FileSystem
            return path
        end

        function blueprint_creator(): Effect<Result<string, string>, string, FileSystem>
            return effect do
                return read_file("test.txt")
            end
        end
    "#;
    assert!(typecheck_program(src_captured_in_effect).is_ok());
}

#[test]
fn test_try_unwrap_propagation() {
    let src = r#"
        struct Fs
        end
        service FileSystem: Fs

        function read_file(path: string): string ! string ~ FileSystem
            return path
        end

        function read_and_process(): string ! string ~ FileSystem
            local content = read_file("config.json")?
            return content
        end
    "#;
    assert!(typecheck_program(src).is_ok());
}

