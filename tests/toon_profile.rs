use lust::data::toon::{self, ToonError};
use lust::data::{DataValue, ExactNumber};

#[test]
fn test_toon_decode_and_encode_roundtrip() {
    let input = r#"
[id, name, score, active]
1, Alice, 95.5, true
2, Bob, 82.0, false
3, "Charlie Brown", 100.0, true
"#;

    let res = toon::from_str(input).unwrap();
    if let DataValue::Array(rows) = &res {
        assert_eq!(rows.len(), 3);
        if let DataValue::Document(first) = &rows[0] {
            assert_eq!(first.get("id").unwrap(), &DataValue::Number(ExactNumber::Int(1)));
            assert_eq!(first.get("name").unwrap(), &DataValue::String("Alice".to_string()));
            assert_eq!(first.get("score").unwrap(), &DataValue::Number(ExactNumber::Float(95.5)));
            assert_eq!(first.get("active").unwrap(), &DataValue::Bool(true));
        } else {
            panic!("Expected document in row");
        }
    } else {
        panic!("Expected array of rows");
    }

    let encoded = toon::to_string(&res).unwrap();
    let decoded_again = toon::from_str(&encoded).unwrap();
    assert_eq!(res, decoded_again);
}

#[test]
fn test_toon_column_mismatch_rejected() {
    let invalid = r#"
[id, name]
1, Alice, extra_val
"#;
    let res = toon::from_str(invalid);
    assert!(matches!(res, Err(ToonError::ColumnMismatch { expected: 2, found: 3, row: 1 })));
}

#[test]
fn test_gate_01_toon_duplicate_column_rejected() {
    let duplicate_header = r#"
[id, name, id]
1, Alice, 2
"#;
    let res = toon::from_str(duplicate_header);
    assert!(matches!(res, Err(ToonError::DuplicateColumn(_))));
}
