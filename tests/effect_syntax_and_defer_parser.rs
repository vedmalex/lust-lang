#![cfg(feature = "std")]

use lust::{
    ast::{BinaryOp, ExprKind, ItemKind, StmtKind, TypeKind},
    lexer::Lexer,
    parser::Parser,
};

fn parse_program(source: &str) -> Vec<lust::Item> {
    let mut interner = lust::intern::Interner::new();
    let tokens = Lexer::new(source, &mut interner)
        .tokenize()
        .expect("tokenize failed");
    Parser::new(tokens).parse().expect("parse failed")
}

#[test]
fn test_function_effect_signature_return_spec() {
    let src = r#"
        function load(p: string): string ! IoError ~ (FileSystem + Console)
            return p
        end
    "#;
    let items = parse_program(src);
    assert_eq!(items.len(), 1);

    if let ItemKind::Function(f) = &items[0].kind {
        assert_eq!(f.name, "load");
        let spec = f.return_spec();

        let ret = spec.return_type.as_ref().expect("expected return type");
        assert_eq!(ret.kind, TypeKind::String);

        let err = spec.error_type.as_ref().expect("expected error type");
        assert_eq!(err.kind, TypeKind::Named("IoError".to_string()));

        assert_eq!(spec.requirements.len(), 2);
        assert_eq!(
            spec.requirements[0].kind,
            TypeKind::Named("FileSystem".to_string())
        );
        assert_eq!(
            spec.requirements[1].kind,
            TypeKind::Named("Console".to_string())
        );

        // Also test helper methods
        assert_eq!(f.error_type(), spec.error_type);
        assert_eq!(f.requirements(), spec.requirements);
    } else {
        panic!("expected function item");
    }
}

#[test]
fn test_effect_type_parsing() {
    let src = r#"
        function test_effects(
            a: Effect<int, DbError, Database + Logger>,
            b: Effect<string, IoError>,
            c: Effect<bool>
        )
            return
        end
    "#;
    let items = parse_program(src);
    if let ItemKind::Function(f) = &items[0].kind {
        assert_eq!(f.params.len(), 3);

        // Param a: Effect<int, DbError, Database + Logger>
        if let TypeKind::Effect {
            success,
            error,
            requirements,
        } = &f.params[0].ty.kind
        {
            assert_eq!(success.kind, TypeKind::Int);
            assert_eq!(error.kind, TypeKind::Named("DbError".to_string()));
            assert_eq!(requirements.len(), 2);
            assert_eq!(requirements[0].kind, TypeKind::Named("Database".to_string()));
            assert_eq!(requirements[1].kind, TypeKind::Named("Logger".to_string()));
        } else {
            panic!("param a expected Effect type");
        }

        // Param b: Effect<string, IoError> (requirements empty)
        if let TypeKind::Effect {
            success,
            error,
            requirements,
        } = &f.params[1].ty.kind
        {
            assert_eq!(success.kind, TypeKind::String);
            assert_eq!(error.kind, TypeKind::Named("IoError".to_string()));
            assert!(requirements.is_empty());
        } else {
            panic!("param b expected Effect type");
        }

        // Param c: Effect<bool> (error is Never, requirements empty)
        if let TypeKind::Effect {
            success,
            error,
            requirements,
        } = &f.params[2].ty.kind
        {
            assert_eq!(success.kind, TypeKind::Bool);
            assert_eq!(error.kind, TypeKind::Never);
            assert!(requirements.is_empty());
        } else {
            panic!("param c expected Effect type");
        }
    } else {
        panic!("expected function item");
    }
}

#[test]
fn test_regression_f02_bitxor_tilde_not_absorbed_into_cast() {
    let src = r#"
        function bitxor_test(x: int, y: int)
            local z1 = (x as int) ~ y
            local z2 = x as int ~ y
            return z1
        end
    "#;
    let items = parse_program(src);
    if let ItemKind::Function(f) = &items[0].kind {
        // Statement 1: local z1 = (x as int) ~ y
        if let StmtKind::Local { initializer, .. } = &f.body[0].kind {
            let expr = &initializer.as_ref().unwrap()[0];
            if let ExprKind::Binary { left, op, right } = &expr.kind {
                assert_eq!(*op, BinaryOp::BitXor);
                assert!(matches!(&left.kind, ExprKind::Paren(_)));
                if let ExprKind::Identifier(name) = &right.kind {
                    assert_eq!(name, "y");
                } else {
                    panic!("right side of bitxor must be identifier y");
                }
            } else {
                panic!("z1 expected to parse as BinaryOp::BitXor, got {:?}", expr.kind);
            }
        } else {
            panic!("expected local stmt 0");
        }

        // Statement 2: local z2 = x as int ~ y
        if let StmtKind::Local { initializer, .. } = &f.body[1].kind {
            let expr = &initializer.as_ref().unwrap()[0];
            if let ExprKind::Binary { left, op, right } = &expr.kind {
                assert_eq!(*op, BinaryOp::BitXor);
                assert!(matches!(&left.kind, ExprKind::Cast { .. }));
                if let ExprKind::Identifier(name) = &right.kind {
                    assert_eq!(name, "y");
                } else {
                    panic!("right side of bitxor must be identifier y");
                }
            } else {
                panic!("z2 expected to parse as BinaryOp::BitXor, got {:?}", expr.kind);
            }
        } else {
            panic!("expected local stmt 1");
        }
    } else {
        panic!("expected function item");
    }
}

#[test]
fn test_defer_errdefer_scoped_statements() {
    let src = r#"
        function defer_tests()
            defer file.close()
            defer do
                print("closed")
            end
            defer {
                cleanup()
            }
            errdefer rollback()
            errdefer {
                abort()
            }
            scoped do
                local temp = 123
            end
            scoped {
                local temp2 = 456
            }
        end
    "#;
    let items = parse_program(src);
    if let ItemKind::Function(f) = &items[0].kind {
        assert_eq!(f.body.len(), 7);

        // 0: defer file.close()
        assert!(matches!(&f.body[0].kind, StmtKind::Defer(_)));

        // 1: defer do ... end
        if let StmtKind::Defer(inner) = &f.body[1].kind {
            assert!(matches!(&inner.kind, StmtKind::Block(_)));
        } else {
            panic!("expected defer block");
        }

        // 2: defer { ... }
        if let StmtKind::Defer(inner) = &f.body[2].kind {
            assert!(matches!(&inner.kind, StmtKind::Block(_)));
        } else {
            panic!("expected defer block");
        }

        // 3: errdefer rollback()
        assert!(matches!(&f.body[3].kind, StmtKind::ErrDefer(_)));

        // 4: errdefer { ... }
        if let StmtKind::ErrDefer(inner) = &f.body[4].kind {
            assert!(matches!(&inner.kind, StmtKind::Block(_)));
        } else {
            panic!("expected errdefer block");
        }

        // 5: scoped do ... end
        if let StmtKind::Scoped(stmts) = &f.body[5].kind {
            assert_eq!(stmts.len(), 1);
        } else {
            panic!("expected scoped do block");
        }

        // 6: scoped { ... }
        if let StmtKind::Scoped(stmts) = &f.body[6].kind {
            assert_eq!(stmts.len(), 1);
        } else {
            panic!("expected scoped brace block");
        }
    } else {
        panic!("expected function item");
    }
}

#[test]
fn test_effect_block_lazy_suspension() {
    let src = r#"
        function suspension_test()
            local job1 = effect do
                return 42
            end
            local job2 = effect {
                return 84
            }
            local effect = 100
            return effect
        end
    "#;
    let items = parse_program(src);
    if let ItemKind::Function(f) = &items[0].kind {
        assert_eq!(f.body.len(), 4);

        // job1 = effect do ... end
        if let StmtKind::Local { initializer, .. } = &f.body[0].kind {
            let expr = &initializer.as_ref().unwrap()[0];
            if let ExprKind::EffectBlock(stmts) = &expr.kind {
                assert_eq!(stmts.len(), 1);
            } else {
                panic!("expected EffectBlock for job1");
            }
        } else {
            panic!("expected local job1");
        }

        // job2 = effect { ... }
        if let StmtKind::Local { initializer, .. } = &f.body[1].kind {
            let expr = &initializer.as_ref().unwrap()[0];
            if let ExprKind::EffectBlock(stmts) = &expr.kind {
                assert_eq!(stmts.len(), 1);
            } else {
                panic!("expected EffectBlock for job2");
            }
        } else {
            panic!("expected local job2");
        }

        // local effect = 100 (contextual keyword safety check)
        if let StmtKind::Local { bindings, initializer } = &f.body[2].kind {
            assert_eq!(bindings[0].name, "effect");
            let expr = &initializer.as_ref().unwrap()[0];
            assert!(matches!(&expr.kind, ExprKind::Literal(_)));
        } else {
            panic!("expected local effect = 100");
        }
    } else {
        panic!("expected function item");
    }
}

#[test]
fn test_effect_signatures_with_unit_return_default() {
    let src = r#"
        function foo(): () ! IoError
            return
        end

        function bar() ! IoError ~ Logger
            return
        end

        function baz() ~ Logger
            return
        end
    "#;
    let items = parse_program(src);
    assert_eq!(items.len(), 3);

    // foo(): () ! IoError
    if let ItemKind::Function(f) = &items[0].kind {
        let spec = f.return_spec();
        assert_eq!(spec.return_type.unwrap().kind, TypeKind::Unit);
        assert_eq!(spec.error_type.unwrap().kind, TypeKind::Named("IoError".to_string()));
        assert!(spec.requirements.is_empty());
    }

    // bar() ! IoError ~ Logger (omitted return type defaults to Unit)
    if let ItemKind::Function(f) = &items[1].kind {
        let spec = f.return_spec();
        assert_eq!(spec.return_type.unwrap().kind, TypeKind::Unit);
        assert_eq!(spec.error_type.unwrap().kind, TypeKind::Named("IoError".to_string()));
        assert_eq!(spec.requirements.len(), 1);
        assert_eq!(spec.requirements[0].kind, TypeKind::Named("Logger".to_string()));
    }

    // baz() ~ Logger
    if let ItemKind::Function(f) = &items[2].kind {
        let spec = f.return_spec();
        assert_eq!(spec.return_type.unwrap().kind, TypeKind::Unit);
        assert!(spec.error_type.is_none());
        assert_eq!(spec.requirements.len(), 1);
        assert_eq!(spec.requirements[0].kind, TypeKind::Named("Logger".to_string()));
    }
}

#[test]
fn test_lambda_effect_signature() {
    let src = r#"
        function lambda_test()
            local f = function(x: int): int ! IoError ~ (Fs + Net)
                return x
            end
            return f
        end
    "#;
    let items = parse_program(src);
    if let ItemKind::Function(f) = &items[0].kind {
        if let StmtKind::Local { initializer, .. } = &f.body[0].kind {
            let expr = &initializer.as_ref().unwrap()[0];
            let spec = expr.return_spec().expect("lambda must have return_spec");
            assert_eq!(spec.return_type.unwrap().kind, TypeKind::Int);
            assert_eq!(spec.error_type.unwrap().kind, TypeKind::Named("IoError".to_string()));
            assert_eq!(spec.requirements.len(), 2);
            assert_eq!(spec.requirements[0].kind, TypeKind::Named("Fs".to_string()));
            assert_eq!(spec.requirements[1].kind, TypeKind::Named("Net".to_string()));
        } else {
            panic!("expected local f");
        }
    } else {
        panic!("expected function item");
    }
}

#[test]
fn test_trait_method_effect_signature() {
    let src = r#"
        trait Repo
            function find(id: int): string ! DbError ~ Database
        end
    "#;
    let items = parse_program(src);
    if let ItemKind::Trait(t) = &items[0].kind {
        assert_eq!(t.methods.len(), 1);
        let m = &t.methods[0];
        let spec = m.return_spec();
        assert_eq!(spec.return_type.unwrap().kind, TypeKind::String);
        assert_eq!(spec.error_type.unwrap().kind, TypeKind::Named("DbError".to_string()));
        assert_eq!(spec.requirements.len(), 1);
        assert_eq!(spec.requirements[0].kind, TypeKind::Named("Database".to_string()));
    } else {
        panic!("expected trait item");
    }
}

#[test]
fn test_bang_token_lexer() {
    let src = "x != y ! IoError";
    let mut interner = lust::intern::Interner::new();
    let tokens = Lexer::new(src, &mut interner)
        .tokenize()
        .expect("tokenize failed");
    let kinds: Vec<_> = tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![
            lust::lexer::TokenKind::Identifier,
            lust::lexer::TokenKind::NotEqual,
            lust::lexer::TokenKind::Identifier,
            lust::lexer::TokenKind::Bang,
            lust::lexer::TokenKind::Identifier,
            lust::lexer::TokenKind::Eof,
        ]
    );
}
