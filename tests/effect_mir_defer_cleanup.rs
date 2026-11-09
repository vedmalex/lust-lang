#![cfg(feature = "std")]
use lust::{EmbeddedProgram, Value};

#[test]
fn effect_construction_is_lazy() {
    let source = r#"
        function main(): int
            local job = effect do
                return 1 / 0
            end
            return 42
        end
    "#;
    let mut program = EmbeddedProgram::builder()
        .module("main", source)
        .entry_module("main")
        .compile().unwrap();
    assert_eq!(program.vm_mut().call("main.main", vec![]).unwrap(), Value::Int(42));
}
