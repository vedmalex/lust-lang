use lust::{EmbeddedProgram, intern::Interner, lexer::Lexer, parser::Parser};

#[test]
fn control_small_core_rejects_domain_sugar() {
    for src in ["schema User end", "entity User end", "event Created end"] {
        let mut interner = Interner::new();
        let tokens = Lexer::new(src, &mut interner).tokenize().unwrap();
        assert!(Parser::new(tokens).parse().is_err(), "accepted {src}");
    }
}

#[test]
fn schema_stdlib_is_available_to_lust_compiler() {
    let result = EmbeddedProgram::builder()
        .module("main", "use std.schema\nfunction test(): int return 1 end")
        .entry_module("main").compile();
    assert!(result.is_ok(), "frozen schema library unavailable: {:?}", result.err());
}

#[test]
fn control_existing_stdlib_is_available() {
    let result = EmbeddedProgram::builder()
        .module("main", "use std.math\nfunction test(): int return 1 end")
        .entry_module("main").compile();
    assert!(result.is_ok(), "control: {:?}", result.err());
}

#[test]
fn yaml_aliases_must_not_resolve_from_previous_document() {
    let mut parser = lust::data::yaml::LustSafeYaml12::new().with_opt_in_aliases();
    assert!(parser.parse("a: &old secret").is_ok());
    assert!(parser.parse("b: *old").is_err(), "alias without a definition in this document must fail closed");
}
