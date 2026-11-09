use lust::data::bson::{self, BsonError};
use lust::data::{DataValue, DateTimeUtc, Decimal128, Document, ExactNumber, ObjectId};

#[test]
fn test_bson_roundtrip_all_types() {
    let mut doc = Document::new();
    doc.try_insert("null_key".to_string(), DataValue::Null).unwrap();
    doc.try_insert("bool_true".to_string(), DataValue::Bool(true)).unwrap();
    doc.try_insert("bool_false".to_string(), DataValue::Bool(false)).unwrap();
    doc.try_insert("int32".to_string(), DataValue::Number(ExactNumber::Int(12345))).unwrap();
    doc.try_insert("int64".to_string(), DataValue::Number(ExactNumber::Int(5_000_000_000))).unwrap();
    doc.try_insert("float".to_string(), DataValue::Number(ExactNumber::Float(123.456))).unwrap();
    doc.try_insert("str".to_string(), DataValue::String("Lust BSON test".to_string())).unwrap();
    doc.try_insert("bin".to_string(), DataValue::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF])).unwrap();

    let oid = ObjectId([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    doc.try_insert("oid".to_string(), DataValue::ObjectId(oid)).unwrap();

    let dt = DateTimeUtc(1700000000000);
    doc.try_insert("created_at".to_string(), DataValue::DateTimeUtc(dt)).unwrap();

    let dec = Decimal128([7u8; 16]);
    doc.try_insert("price_dec".to_string(), DataValue::Number(ExactNumber::Decimal128(dec))).unwrap();

    let encoded = bson::to_vec(&doc);
    let decoded = bson::from_slice(&encoded).unwrap();

    assert_eq!(doc, decoded);
}

#[test]
fn test_bson_nested_document_and_array() {
    let mut inner = Document::new();
    inner.try_insert("sub_k".to_string(), DataValue::String("sub_v".to_string())).unwrap();

    let mut doc = Document::new();
    doc.try_insert("nested".to_string(), DataValue::Document(inner)).unwrap();
    doc.try_insert(
        "items".to_string(),
        DataValue::Array(vec![
            DataValue::Number(ExactNumber::Int(1)),
            DataValue::Number(ExactNumber::Int(2)),
            DataValue::Number(ExactNumber::Int(3)),
        ]),
    ).unwrap();

    let encoded = bson::to_vec(&doc);
    let decoded = bson::from_slice(&encoded).unwrap();

    assert_eq!(doc, decoded);
}

#[test]
fn test_bson_reject_duplicate_key() {
    // Manually craft a BSON buffer containing two identical keys "k"
    let mut raw = Vec::new();
    // 4 bytes length placeholder
    raw.extend_from_slice(&[0u8; 4]);

    // Element 1: int32 "k" = 10
    raw.push(0x10);
    raw.extend_from_slice(b"k\0");
    raw.extend_from_slice(&10i32.to_le_bytes());

    // Element 2: int32 duplicate "k" = 20
    raw.push(0x10);
    raw.extend_from_slice(b"k\0");
    raw.extend_from_slice(&20i32.to_le_bytes());

    // Document terminator
    raw.push(0x00);

    let len = raw.len() as i32;
    raw[0..4].copy_from_slice(&len.to_le_bytes());

    let res = bson::from_slice(&raw);
    assert!(res.is_err());
    match res.unwrap_err() {
        BsonError::DuplicateKey(k) => assert_eq!(k, "k"),
        other => panic!("Expected DuplicateKey error, got {:?}", other),
    }
}

#[test]
fn test_bson_malformed_suffix() {
    let mut doc = Document::new();
    doc.try_insert("k".to_string(), DataValue::Number(ExactNumber::Int(1))).unwrap();
    let mut bytes = bson::to_vec(&doc);
    // append trailing junk bytes
    bytes.extend_from_slice(b"extra_junk");

    let res = bson::from_slice(&bytes);
    assert_eq!(res.unwrap_err(), BsonError::MalformedSuffix);
}

#[test]
fn test_bson_missing_null_terminator() {
    let mut doc = Document::new();
    doc.try_insert("k".to_string(), DataValue::Number(ExactNumber::Int(1))).unwrap();
    let mut bytes = bson::to_vec(&doc);
    // corrupt last byte (which must be 0x00)
    let last = bytes.len() - 1;
    bytes[last] = 0xFF;

    let res = bson::from_slice(&bytes);
    assert_eq!(res.unwrap_err(), BsonError::MissingNullTerminator);
}
