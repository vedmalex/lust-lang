#![cfg(feature = "std")]
use lust::{lexer::Lexer, parser::Parser, typechecker::TypeChecker};
fn check(source: &str) -> lust::error::Result<()> {
    let mut interner = lust::intern::Interner::new();
    let tokens = Lexer::new(source, &mut interner).tokenize()?;
    let items = Parser::new(tokens).parse()?;
    TypeChecker::new().check_module(&items)
}
#[test]
fn effect_run_sync_without_requirements() {
    check("function f(): int return (effect do return 42 end).run_sync() end").unwrap();
}
#[test]
fn effect_run_sync_rejects_requirements() {
    let error = check("struct S end function f(e: Effect<int, never, S>): int return e.run_sync() end").unwrap_err();
    assert!(error.to_string().contains("unhandled requirements"), "{error}");
}
#[test]
fn provide_discharges_nominal_service() {
    check("struct S end function f(e: Effect<int, never, S>, s: S): int return e.provide(s).run_sync() end").unwrap();
}
#[test]
fn provide_wrong_nominal_service_fails() {
    let error = check("struct S end struct T end function f(e: Effect<int, never, S>, t: T): int return e.provide(t).run_sync() end").unwrap_err();
    assert!(error.to_string().contains("unhandled requirements"), "{error}");
}
#[test]
fn cleanup_return_rejected() {
    assert!(check("function f(): int defer return 1 return 2 end").unwrap_err().to_string().contains("not allowed inside defer"));
}
