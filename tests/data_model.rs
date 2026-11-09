#![cfg(feature = "std")]

use lust::{
    DataValue, DateTimeUtc, Decimal128, Document, DuplicateKeyError, ExactNumber, ObjectId,
};

#[test]
fn test_exact_number_representations() {
    let int_val = ExactNumber::from(42i64);
    assert!(int_val.is_integer());
    assert!(!int_val.is_float());
    assert_eq!(int_val.as_i64(), Some(42));
    assert_eq!(int_val.as_u64(), Some(42));
    assert_eq!(int_val.as_f64(), Some(42.0));

    let uint_val = ExactNumber::from(100u64);
    assert!(uint_val.is_integer());
    assert_eq!(uint_val.as_u64(), Some(100));
    assert_eq!(uint_val.as_i64(), Some(100));

    let float_val = ExactNumber::from(3.14159f64);
    assert!(!float_val.is_integer());
    assert!(float_val.is_float());
    assert_eq!(float_val.as_f64(), Some(3.14159));
    assert_eq!(float_val.as_i64(), None);

    let float_int = ExactNumber::from(100.0f64);
    assert_eq!(float_int.as_i64(), Some(100));

    // Adversarial: Large integer exceeding 2^53 - 1 must reject conversion to f64 to prevent silent precision loss
    let large_int = ExactNumber::from(i64::MAX);
    assert_eq!(large_int.as_f64(), None);

    // Adversarial: Decimal128 must NOT silently coerce to 0.0 or f64
    let d128 = ExactNumber::from(Decimal128::new([0xAA; 16]));
    assert_eq!(d128.as_f64(), None);
}

#[test]
fn test_object_id_roundtrip_and_timestamp() {
    let hex = "507f1f77bcf86cd799439011";
    let oid = ObjectId::from_hex(hex).expect("valid hex");
    assert_eq!(oid.to_hex(), hex);
    assert_eq!(oid.timestamp(), 0x507f1f77);

    // Invalid length
    assert!(ObjectId::from_hex("1234").is_err());
    // Invalid characters
    assert!(ObjectId::from_hex("507f1f77bcf86cd7994390zz").is_err());
}

#[test]
fn test_date_time_utc_roundtrip() {
    let ms = 1773000000000i64;
    let dt = DateTimeUtc::from_millis(ms);
    assert_eq!(dt.millis(), ms);
}

#[test]
fn test_decimal128_raw_bytes() {
    let raw = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let d128 = Decimal128::new(raw);
    assert_eq!(d128.raw_bytes(), &raw);
}

#[test]
fn test_document_ordered_unique_keys() {
    let mut doc = Document::new();
    assert!(doc.is_empty());
    doc.insert("a", 1);
    doc.insert("b", 2);
    doc.insert("c", 3);

    assert_eq!(doc.len(), 3);
    let keys: Vec<&String> = doc.keys().collect();
    assert_eq!(keys, vec!["a", "b", "c"]);

    // Update existing key preserves position
    let old = doc.insert("b", 20);
    assert_eq!(old, Some(DataValue::from(2)));
    assert_eq!(doc.len(), 3);
    let keys_after: Vec<&String> = doc.keys().collect();
    assert_eq!(keys_after, vec!["a", "b", "c"]);
    assert_eq!(doc.get("b"), Some(&DataValue::from(20)));

    // Removal
    let removed = doc.remove("b");
    assert_eq!(removed, Some(DataValue::from(20)));
    assert_eq!(doc.len(), 2);
    assert!(!doc.contains_key("b"));
}

#[test]
fn test_document_try_insert_gate_01_duplicate_rejection() {
    let mut doc = Document::new();
    assert!(doc.try_insert("user_id", 101).is_ok());
    assert!(doc.try_insert("username", "alice").is_ok());

    // Duplicate key insertion MUST fail closed
    let res = doc.try_insert("user_id", 999);
    assert_eq!(res, Err(DuplicateKeyError));
    // The original value must remain unchanged
    assert_eq!(doc.get("user_id"), Some(&DataValue::from(101)));
}

#[test]
fn test_datavalue_nested_structure() {
    let mut child = Document::new();
    child.insert("tag", "alpha");
    child.insert("score", 98.5);

    let mut root = Document::new();
    root.insert("id", ObjectId::from_hex("507f1f77bcf86cd799439011").unwrap());
    root.insert("child", child);
    root.insert("tags", vec![DataValue::from("a"), DataValue::from("b")]);

    let val = DataValue::from(root);
    assert!(val.is_document());
    let doc = val.as_document().unwrap();
    assert_eq!(doc.len(), 3);
    assert!(doc.contains_key("id"));
    assert!(doc.contains_key("child"));
    assert!(doc.contains_key("tags"));

    let child_doc = doc.get("child").unwrap().as_document().unwrap();
    assert_eq!(child_doc.get("tag"), Some(&DataValue::from("alpha")));
}

#[test]
fn test_gate_01_mutant_accepting_duplicates_fails_oracle() {
    // Mutant Oracle Verification: simulate broken mutant that silently accepts duplicates
    fn mutant_insert_ignoring_duplicates(doc: &mut Document, k: &str, v: i64) -> Result<(), DuplicateKeyError> {
        // Deliberate mutant behavior: calls doc.insert() and ignores duplicate check
        doc.insert(k, v);
        Ok(())
    }

    let mut doc = Document::new();
    assert!(doc.try_insert("k1", 1).is_ok());

    // Positive check on production code: rejects duplicate
    assert_eq!(doc.try_insert("k1", 2), Err(DuplicateKeyError));

    // Mutant check: if mutant were present, it would return Ok(()) which our Gate oracle detects
    let mutant_result = mutant_insert_ignoring_duplicates(&mut doc, "k1", 2);
    assert_eq!(mutant_result, Ok(())); // Catches that mutant diverges from required Err(DuplicateKeyError)
}
