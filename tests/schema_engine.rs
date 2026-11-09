use lust::data::json;
use lust::data::{DataValue, ExactNumber};
use lust::schema::s;

#[test]
fn test_schema_primitive_string() {
    let username_schema = s.string().min(3).max(10).build();

    let valid = DataValue::String("alex".to_string());
    assert_eq!(username_schema.parse(&valid).unwrap(), valid);

    let too_short = DataValue::String("al".to_string());
    let err = username_schema.parse(&too_short).unwrap_err();
    assert_eq!(err.errors[0].code, "too_small");
    assert_eq!(err.errors[0].path_string(), "$");

    let wrong_type = DataValue::Number(ExactNumber::Int(123));
    let err = username_schema.parse(&wrong_type).unwrap_err();
    assert_eq!(err.errors[0].code, "invalid_type");
}

#[test]
fn test_schema_primitive_numbers() {
    let age_schema = s.int().min(0).max(120).build();

    let valid = DataValue::Number(ExactNumber::Int(25));
    assert_eq!(age_schema.parse(&valid).unwrap(), valid);

    let negative = DataValue::Number(ExactNumber::Int(-5));
    let err = age_schema.parse(&negative).unwrap_err();
    assert_eq!(err.errors[0].code, "too_small");

    let too_old = DataValue::Number(ExactNumber::Int(150));
    let err = age_schema.parse(&too_old).unwrap_err();
    assert_eq!(err.errors[0].code, "too_big");
}

#[test]
fn test_schema_nested_object_exact_path() {
    let address_schema = s
        .object()
        .field("street", s.string().min(1))
        .field("city", s.string().min(1))
        .field("zip", s.string().min(5).max(10))
        .build();

    let user_schema = s
        .object()
        .field("name", s.string().min(2))
        .field("address", address_schema)
        .build();

    let json_input = r#"{
        "name": "Bob",
        "address": {
            "street": "123 Main St",
            "city": "Metropolis",
            "zip": "99"
        }
    }"#;

    let parsed_val = json::from_str(json_input).unwrap();
    let res = user_schema.parse(&parsed_val);
    assert!(res.is_err());
    let errors = res.unwrap_err().errors;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path_string(), "address.zip");
    assert_eq!(errors[0].code, "too_small");
}

#[test]
fn test_schema_optional_and_default_fields() {
    let config_schema = s
        .object()
        .field("host", s.string().min(1))
        .field_with_default("port", s.int().build(), DataValue::Number(ExactNumber::Int(8080)))
        .field("debug", s.bool().optional())
        .build();

    let json_input = r#"{"host": "localhost"}"#;
    let parsed_val = json::from_str(json_input).unwrap();
    let validated = config_schema.parse(&parsed_val).unwrap();

    if let DataValue::Document(doc) = validated {
        assert_eq!(doc.get("host").unwrap(), &DataValue::String("localhost".to_string()));
        assert_eq!(doc.get("port").unwrap(), &DataValue::Number(ExactNumber::Int(8080)));
        assert_eq!(doc.get("debug"), None);
    } else {
        panic!("Expected document");
    }
}

#[test]
fn test_schema_strict_object_rejects_extra_keys() {
    let strict_schema = s
        .object()
        .field("id", s.int())
        .strict()
        .build();

    let json_input = r#"{"id": 1, "extra": "unwelcome"}"#;
    let parsed_val = json::from_str(json_input).unwrap();
    let err = strict_schema.parse(&parsed_val).unwrap_err();
    assert_eq!(err.errors[0].code, "unrecognized_key");
    assert_eq!(err.errors[0].path_string(), "extra");
}

#[test]
fn test_schema_array_elements_path() {
    let order_schema = s
        .object()
        .field("order_id", s.string())
        .field("items", s.array(s.object().field("sku", s.string().min(3)).field("price", s.float().min(0.0))))
        .build();

    let json_input = r#"{
        "order_id": "ORD-1",
        "items": [
            {"sku": "ABC", "price": 10.5},
            {"sku": "X", "price": 5.0}
        ]
    }"#;

    let parsed_val = json::from_str(json_input).unwrap();
    let err = order_schema.parse(&parsed_val).unwrap_err();
    assert_eq!(err.errors[0].path_string(), "items.1.sku");
    assert_eq!(err.errors[0].code, "too_small");
}

#[test]
fn test_schema_discriminated_union() {
    let circle_schema = s
        .object()
        .field("type", s.literal(DataValue::String("circle".to_string())))
        .field("radius", s.float().min(0.0))
        .build();

    let rect_schema = s
        .object()
        .field("type", s.literal(DataValue::String("rect".to_string())))
        .field("width", s.float().min(0.0))
        .field("height", s.float().min(0.0))
        .build();

    let shape_schema = s.discriminated_union("type", vec![circle_schema, rect_schema]);

    let circle_json = r#"{"type": "circle", "radius": 5.5}"#;
    let circle_val = json::from_str(circle_json).unwrap();
    assert!(shape_schema.parse(&circle_val).is_ok());

    let rect_json = r#"{"type": "rect", "width": 10.0, "height": 20.0}"#;
    let rect_val = json::from_str(rect_json).unwrap();
    assert!(shape_schema.parse(&rect_val).is_ok());

    let invalid_json = r#"{"type": "triangle", "base": 1.0}"#;
    let invalid_val = json::from_str(invalid_json).unwrap();
    let err = shape_schema.parse(&invalid_val).unwrap_err();
    assert_eq!(err.errors[0].code, "invalid_discriminator");
}

#[test]
fn test_schema_fluent_refine_and_transform() {
    let username_schema = s
        .string()
        .min(3)
        .max(20)
        .refine(|val| match val {
            DataValue::String(str_val) => str_val.starts_with("user_"),
            _ => false,
        })
        .transform(|val| match val {
            DataValue::String(str_val) => DataValue::String(str_val.to_uppercase()),
            other => other,
        });

    let valid = DataValue::String("user_alex".to_string());
    let parsed = username_schema.parse(&valid).unwrap();
    assert_eq!(parsed, DataValue::String("USER_ALEX".to_string()));

    let invalid_prefix = DataValue::String("admin_alex".to_string());
    let err = username_schema.parse(&invalid_prefix).unwrap_err();
    assert_eq!(err.errors[0].code, "custom_refinement");

    let too_short = DataValue::String("u".to_string());
    let err = username_schema.parse(&too_short).unwrap_err();
    assert_eq!(err.errors[0].code, "too_small");
}

#[test]
fn test_schema_int_refine_and_transform() {
    let even_positive = s
        .int()
        .min(1)
        .refine_with_message(
            |val| match val {
                DataValue::Number(ExactNumber::Int(i)) => i % 2 == 0,
                _ => false,
            },
            "Must be an even integer",
        )
        .transform(|val| match val {
            DataValue::Number(ExactNumber::Int(i)) => DataValue::Number(ExactNumber::Int(i * 10)),
            other => other,
        });

    let valid = DataValue::Number(ExactNumber::Int(4));
    let parsed = even_positive.parse(&valid).unwrap();
    assert_eq!(parsed, DataValue::Number(ExactNumber::Int(40)));

    let odd = DataValue::Number(ExactNumber::Int(5));
    let err = even_positive.parse(&odd).unwrap_err();
    assert_eq!(err.errors[0].code, "custom_refinement");
    assert_eq!(err.errors[0].message, "Must be an even integer");
}

#[test]
fn test_schema_compile_to_schema_ir() {
    use lust::schema::SchemaIR;

    let str_builder = s.string().min(5).max(50);
    let ir: SchemaIR = str_builder.compile();
    assert!(matches!(ir, SchemaIR::String { min_len: Some(5), max_len: Some(50), regex: None }));

    let schema = s.object().field("id", s.int().min(1)).build();
    let obj_ir = schema.compile();
    assert!(matches!(obj_ir, SchemaIR::Object { fields, strict: false } if fields.len() == 1));
}

#[test]
fn test_schema_validation_returns_effect() {
    let schema = s.string().min(2).max(10).build();

    let valid = DataValue::String("hello".to_string());
    let effect = schema.validate(&valid);
    assert!(effect.is_success());
    assert!(!effect.is_failure());
    assert_eq!(effect.exit, lust::schema::Exit::Success(valid.clone()));

    let invalid = DataValue::String("x".to_string());
    let effect = schema.validate(&invalid);
    assert!(effect.is_failure());
    assert!(!effect.is_success());
    match &effect.exit {
        lust::schema::Exit::Failure(lust::schema::Cause::Fail(errors)) => {
            assert_eq!(errors.errors[0].code, "too_small");
        }
        other => panic!("Expected Failure(Cause::Fail(_)), got {:?}", other),
    }

    // Single error effect
    let single_effect = schema.validate_single(&invalid);
    assert!(single_effect.is_failure());
    match &single_effect.exit {
        lust::schema::Exit::Failure(lust::schema::Cause::Fail(err)) => {
            assert_eq!(err.code, "too_small");
        }
        other => panic!("Expected Failure(Cause::Fail(_)), got {:?}", other),
    }
}

#[test]
fn test_schema_invalid_regex_fails_closed_at_build() {
    use lust::schema::SchemaIR;

    let malformed_schema = s.string().regex("[").build();
    assert!(matches!(malformed_schema.ir, SchemaIR::InvalidRegex { ref pattern, .. } if pattern == "["));

    let val = DataValue::String("test string".to_string());
    let effect = malformed_schema.validate(&val);
    assert!(effect.is_failure());
    match &effect.exit {
        lust::schema::Exit::Failure(lust::schema::Cause::Fail(errs)) => {
            assert_eq!(errs.errors[0].code, "invalid_regex");
        }
        other => panic!("Expected invalid_regex failure, got {:?}", other),
    }
}
