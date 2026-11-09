#![cfg(feature = "std")]

use lust::{
    EmbeddedProgram, Value,
    ast::{ExprKind, ItemKind, StmtKind},
    intern::Interner,
    lexer::{Lexer, TokenKind},
    parser::Parser,
};

fn run_vm(source: &str, func: &str) -> lust::Result<Value> {
    let mut prog = EmbeddedProgram::builder()
        .module("main", source)
        .entry_module("main")
        .compile()?;
    prog.vm_mut().call(&format!("main.{}", func), vec![])
}

fn parse_program(source: &str) -> Vec<lust::ast::Item> {
    let mut interner = Interner::new();
    let tokens = Lexer::new(source, &mut interner)
        .tokenize()
        .expect("tokenization should succeed");
    Parser::new(tokens)
        .parse()
        .expect("parsing should succeed")
}

#[test]
fn test_pipe_lexer_tokens() {
    let mut interner = Interner::new();
    let source = "| |= |> |> |";
    let tokens = Lexer::new(source, &mut interner)
        .tokenize()
        .expect("tokenization");
    let kinds: Vec<TokenKind> = tokens.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::Pipe,
            TokenKind::PipeEqual,
            TokenKind::PipeGreater,
            TokenKind::PipeGreater,
            TokenKind::Pipe,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_pipe_ast_desugaring_unary_and_call() {
    let source = r#"
        function test()
            local a = x |> f
            local b = x |> g(1, 2)
            local c = x |> f |> g(1)
        end
    "#;
    let items = parse_program(source);
    assert_eq!(items.len(), 1);

    if let ItemKind::Function(ref f) = items[0].kind {
        let body = &f.body;
        assert_eq!(body.len(), 3);

        // a = x |> f  => Call { callee: f, args: [x] }
        if let StmtKind::Local { initializer: Some(ref inits), .. } = body[0].kind {
            if let ExprKind::Call { ref callee, ref args, .. } = inits[0].kind {
                assert!(matches!(callee.kind, ExprKind::Identifier(ref name) if name == "f"));
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].kind, ExprKind::Identifier(ref name) if name == "x"));
            } else {
                panic!("Expected Call expression for a");
            }
        } else {
            panic!("Expected Local statement for a");
        }

        // b = x |> g(1, 2) => Call { callee: g, args: [x, 1, 2] }
        if let StmtKind::Local { initializer: Some(ref inits), .. } = body[1].kind {
            if let ExprKind::Call { ref callee, ref args, .. } = inits[0].kind {
                assert!(matches!(callee.kind, ExprKind::Identifier(ref name) if name == "g"));
                assert_eq!(args.len(), 3);
                assert!(matches!(args[0].kind, ExprKind::Identifier(ref name) if name == "x"));
            } else {
                panic!("Expected Call expression for b");
            }
        } else {
            panic!("Expected Local statement for b");
        }

        // c = x |> f |> g(1) => Call { callee: g, args: [Call { callee: f, args: [x] }, 1] }
        if let StmtKind::Local { initializer: Some(ref inits), .. } = body[2].kind {
            if let ExprKind::Call { ref callee, ref args, .. } = inits[0].kind {
                assert!(matches!(callee.kind, ExprKind::Identifier(ref name) if name == "g"));
                assert_eq!(args.len(), 2);
                if let ExprKind::Call { callee: ref inner_callee, args: ref inner_args, .. } = args[0].kind {
                    assert!(matches!(inner_callee.kind, ExprKind::Identifier(ref name) if name == "f"));
                    assert_eq!(inner_args.len(), 1);
                } else {
                    panic!("Expected inner Call expression for c's first argument");
                }
            } else {
                panic!("Expected Call expression for c");
            }
        } else {
            panic!("Expected Local statement for c");
        }
    } else {
        panic!("Expected Function item");
    }
}

#[test]
fn test_pipe_operator_vm_unary() {
    let source = r#"
        function double(x: int): int
            return x * 2
        end

        function test(): int
            return 21 |> double
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    assert_eq!(res, Value::Int(42));
}

#[test]
fn test_pipe_operator_vm_thread_first_with_args() {
    let source = r#"
        function add(a: int, b: int): int
            return a + b
        end

        function sub(a: int, b: int): int
            return a - b
        end

        function test(): int
            return 50 |> sub(20) |> add(12)
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    // 50 |> sub(20) => sub(50, 20) = 30
    // 30 |> add(12) => add(30, 12) = 42
    assert_eq!(res, Value::Int(42));
}

#[test]
fn test_pipe_operator_vm_multiline_pipeline() {
    let source = r#"
        function double(x: int): int
            return x * 2
        end

        function add(a: int, b: int): int
            return a + b
        end

        function sub(a: int, b: int): int
            return a - b
        end

        function test(): int
            local res = 10
                |> double
                |> add(5)
                |> sub(3)
            return res
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    // 10 * 2 = 20; 20 + 5 = 25; 25 - 3 = 22
    assert_eq!(res, Value::Int(22));
}

#[test]
fn test_pipe_operator_vm_precedence_arithmetic_first() {
    let source = r#"
        function double(x: int): int
            return x * 2
        end

        function test(): int
            return 2 + 3 |> double
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    // (2 + 3) |> double => double(5) = 10
    assert_eq!(res, Value::Int(10));
}

#[test]
fn test_pipe_operator_vm_lambda_pipeline() {
    let source = r#"
        function apply_twice(x: int, f: function(int): int): int
            return f(f(x))
        end

        function test(): int
            local inc = function(n: int): int return n + 1 end
            return 10 |> apply_twice(inc)
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    // 10 |> apply_twice(inc) => apply_twice(10, inc) => inc(inc(10)) = 12
    assert_eq!(res, Value::Int(12));
}

#[test]
fn test_pipe_operator_vm_direct_anonymous_lambda() {
    let source = r#"
        function test(): int
            return 10 |> (function(x: int): int return x * 3 end)
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    assert_eq!(res, Value::Int(30));
}

#[test]
fn test_pipe_operator_vm_long_mixed_chain() {
    let source = r#"
        function double(x: int): int return x * 2 end
        function add(a: int, b: int): int return a + b end
        function mul(a: int, b: int): int return a * b end
        function sub(a: int, b: int): int return a - b end

        function test(): int
            return 2 |> double |> add(5) |> mul(3) |> sub(1)
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    // (((2 * 2) + 5) * 3) - 1 = (9 * 3) - 1 = 26
    assert_eq!(res, Value::Int(26));
}

#[test]
fn test_pipe_operator_vm_logical_expression_pipe() {
    let source = r#"
        function bool_to_int(b: bool): int
            if b then
                return 1
            else
                return 0
            end
        end

        function test(): int
            return (5 > 2 and 10 < 20) |> bool_to_int
        end
    "#;
    let res = run_vm(source, "test").expect("execution should succeed");
    assert_eq!(res, Value::Int(1));
}

