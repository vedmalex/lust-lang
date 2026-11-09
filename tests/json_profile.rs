use lust::data::json::{self, JsonError};
use lust::data::{DataValue, ExactNumber};

#[test]
fn test_json_strict_scalar_primitives() {
    assert_eq!(json::from_str("null").unwrap(), DataValue::Null);
    assert_eq!(json::from_str("true").unwrap(), DataValue::Bool(true));
    assert_eq!(json::from_str("false").unwrap(), DataValue::Bool(false));
    assert_eq!(
        json::from_str("\"hello world\"").unwrap(),
        DataValue::String("hello world".to_string())
    );
    assert_eq!(
        json::from_str("42").unwrap(),
        DataValue::Number(ExactNumber::Int(42))
    );
    assert_eq!(
        json::from_str("-100").unwrap(),
        DataValue::Number(ExactNumber::Int(-100))
    );
    assert_eq!(
        json::from_str("3.1415").unwrap(),
        DataValue::Number(ExactNumber::Float(3.1415))
    );
    assert_eq!(
        json::from_str("1e6").unwrap(),
        DataValue::Number(ExactNumber::Float(1000000.0))
    );
}

#[test]
fn test_json_unicode_escapes_and_surrogates() {
    // Basic \uXXXX
    let val = json::from_str("\"\\u0041\\u0042\\u0043\"").unwrap();
    assert_eq!(val, DataValue::String("ABC".to_string()));

    // Surrogate pair: \uD83D\uDE00 is 😀 (U+1F600)
    let val = json::from_str("\"\\uD83D\\uDE00\"").unwrap();
    assert_eq!(val, DataValue::String("😀".to_string()));
}

#[test]
fn test_gate_01_json_duplicate_key_rejection() {
    let duplicate_json = r#"{"title": "test", "score": 10, "title": "duplicate"}"#;
    let res = json::from_str(duplicate_json);
    assert!(res.is_err());
    match res.unwrap_err() {
        JsonError::DuplicateKey(key, _) => {
            assert_eq!(key, "title");
        }
        other => panic!("Expected DuplicateKey error, got {:?}", other),
    }
}

#[test]
fn test_json_trailing_comma_rejected_in_strict() {
    let trailing_arr = "[1, 2, 3,]";
    assert!(matches!(
        json::from_str(trailing_arr),
        Err(JsonError::TrailingComma(_))
    ));

    let trailing_obj = r#"{"a": 1, "b": 2,}"#;
    assert!(matches!(
        json::from_str(trailing_obj),
        Err(JsonError::TrailingComma(_))
    ));
}

#[test]
fn test_json_trailing_bytes_rejected() {
    let trailing = r#"{"a": 1} extra_tokens"#;
    assert!(matches!(
        json::from_str(trailing),
        Err(JsonError::TrailingBytes(_))
    ));
}

#[test]
fn test_json_leading_zeros_rejected() {
    let invalid_num = "0123";
    assert!(matches!(
        json::from_str(invalid_num),
        Err(JsonError::InvalidNumber(_))
    ));
}

#[test]
fn test_json_depth_limit() {
    // 5 levels of nesting with limit = 3
    let deeply_nested = "[[[[[1]]]]]";
    let res = json::from_slice_with_limits(deeply_nested.as_bytes(), 3, 1024);
    assert!(matches!(res, Err(JsonError::MaxDepthExceeded(3))));
}

#[test]
fn test_json_max_bytes_limit() {
    let large = "{\"a\": 12345}";
    let res = json::from_slice_with_limits(large.as_bytes(), 10, 5);
    assert!(matches!(res, Err(JsonError::MaxBytesExceeded(5))));
}

#[test]
fn test_json_roundtrip_equality() {
    let json_text = r#"{"name":"Lust Language","version":3,"active":true,"tags":["fast","safe","embeddable"],"metadata":{"score":99.5}}"#;
    let parsed = json::from_str(json_text).unwrap();
    let serialized = json::to_string(&parsed).unwrap();
    let reparsed = json::from_str(&serialized).unwrap();
    assert_eq!(parsed, reparsed);
}
