#![cfg(feature = "std")]

use lust::config::LustConfig;
use lust::builtins::{canonical_std_module_name, is_virtual_std_module, resolve_std_function};
use lust::data::bson::{self, BsonDocumentView, BsonError, BsonValueView};
use lust::data::toon::{self, ToonTable};
use lust::data::yaml::{self, LustSafeYaml12, YamlError};
use lust::data::{DataValue, DateTimeUtc, Decimal128, Document, ExactNumber, ObjectId};

// ============================================================================
// 1. @std/bson: Zero-copy offset view, boundary check, malformed suffix, nominal types
// ============================================================================

#[test]
fn test_12d_bson_zero_copy_view_without_full_deserialize() {
    let hex_id = "507f1f77bcf86cd799439011";
    let oid = ObjectId::from_hex(hex_id).unwrap();
    let dt = DateTimeUtc::from_millis(1700000000000);

    let mut inner = Document::new();
    inner.try_insert("sub_key".to_string(), DataValue::String("sub_val".to_string())).unwrap();

    let mut doc = Document::new();
    doc.try_insert("name".to_string(), DataValue::String("Lust".to_string())).unwrap();
    doc.try_insert("count".to_string(), DataValue::Number(ExactNumber::Int(42))).unwrap();
    doc.try_insert("id".to_string(), DataValue::ObjectId(oid)).unwrap();
    doc.try_insert("created_at".to_string(), DataValue::DateTimeUtc(dt)).unwrap();
    doc.try_insert("nested".to_string(), DataValue::Document(inner)).unwrap();

    let bytes = bson::to_vec(&doc);

    // Create zero-copy view directly over buffer
    let view = BsonDocumentView::from_slice(&bytes).expect("valid BSON view");

    assert_eq!(view.len(), 5);
    assert_eq!(view.get_str("name"), Some("Lust"));
    assert_eq!(view.get_i64("count"), Some(42));
    assert_eq!(view.get_object_id("id"), Some(oid));
    assert_eq!(view.get_datetime("created_at"), Some(dt));

    // Zero-copy nested document view
    let nested_view = view.get_document("nested").expect("nested document view");
    assert_eq!(nested_view.get_str("sub_key"), Some("sub_val"));

    // Materialize only on explicit request
    let roundtrip_doc = view.to_document().expect("materialized document");
    assert_eq!(roundtrip_doc, doc);
}

#[test]
fn test_12d_bson_nested_document_cannot_claim_bytes_past_parent() {
    // Malformed document where nested document claims 20 bytes but parent ends earlier
    let bytes = vec![
        11, 0, 0, 0, // parent declared length: 11 bytes
        0x03, b'n', 0, // embedded document element
        20, 0, 0, 0, // child declared length: 20 bytes (exceeds parent!)
        0x10, b'x', 0, 1, 0, 0, 0,
        0, // child terminator
        0, // outer terminator
    ];

    let view_res = BsonDocumentView::from_slice(&bytes);
    assert!(view_res.is_err(), "nested document claiming bytes past parent must fail closed");

    let from_slice_res = bson::from_slice(&bytes);
    assert!(from_slice_res.is_err(), "bson::from_slice must fail closed on boundary violation");
}

#[test]
fn test_12d_bson_malformed_suffix_fails_closed() {
    let mut doc = Document::new();
    doc.try_insert("k".to_string(), DataValue::String("v".to_string())).unwrap();
    let mut bytes = bson::to_vec(&doc);

    // Append extra junk trailing byte
    bytes.push(0xFF);

    let view_res = BsonDocumentView::from_slice(&bytes);
    assert_eq!(view_res.unwrap_err(), BsonError::MalformedSuffix);

    let from_slice_res = bson::from_slice(&bytes);
    assert_eq!(from_slice_res.unwrap_err(), BsonError::MalformedSuffix);
}

#[test]
fn test_12d_bson_nominal_object_id_and_datetime() {
    let hex = "654321654321654321654321";
    let oid = ObjectId::from_hex(hex).unwrap();
    let dt = DateTimeUtc::from_millis(1234567890123);
    let d128 = Decimal128::new([7u8; 16]);

    let mut doc = Document::new();
    doc.try_insert("oid".to_string(), DataValue::ObjectId(oid)).unwrap();
    doc.try_insert("dt".to_string(), DataValue::DateTimeUtc(dt)).unwrap();
    doc.try_insert("d128".to_string(), DataValue::Number(ExactNumber::Decimal128(d128))).unwrap();

    let bytes = bson::to_vec(&doc);
    let view = bson::view(&bytes).unwrap();

    match view.get("oid").unwrap() {
        BsonValueView::ObjectId(o) => assert_eq!(o, oid),
        other => panic!("Expected nominal ObjectId, got {:?}", other),
    }

    match view.get("dt").unwrap() {
        BsonValueView::DateTimeUtc(d) => assert_eq!(d, dt),
        other => panic!("Expected nominal DateTimeUtc, got {:?}", other),
    }

    match view.get("d128").unwrap() {
        BsonValueView::Decimal128(d) => assert_eq!(d, d128),
        other => panic!("Expected nominal Decimal128, got {:?}", other),
    }
}

// ============================================================================
// 2. @std/yaml: LustSafeYaml12 Budgets, 10x Expansion, Multi-doc Anchors, Junk Rejection
// ============================================================================

#[test]
fn test_12d_yaml_amplification_exceeding_10x_fails_closed() {
    // Bomb attempting 25x expansion
    let mut bomb = String::from("base: &a short_str\n");
    for i in 0..25 {
        bomb.push_str(&format!("k{}: *a\n", i));
    }

    let mut parser = LustSafeYaml12::new().with_opt_in_aliases();
    let res = parser.parse(&bomb);
    assert!(res.is_err());
    match res.unwrap_err() {
        YamlError::AmplificationLimitExceeded { ratio, .. }
        | YamlError::NodeAmplificationLimitExceeded { ratio, .. } => {
            assert!(ratio > 10.0, "Expected ratio > 10x, got {}", ratio);
        }
        other => panic!("Expected amplification error, got {:?}", other),
    }
}

#[test]
fn test_12d_yaml_anchors_must_not_resolve_across_documents() {
    let multi_doc = r#"
---
doc1_key: &shared_anchor hello_doc1
---
doc2_key: *shared_anchor
"#;

    let mut parser = LustSafeYaml12::new().with_opt_in_aliases();
    let res = parser.parse_documents(multi_doc);
    assert!(
        res.is_err(),
        "Anchors must not resolve across documents: got {:?}",
        res
    );
    match res.unwrap_err() {
        YamlError::CrossDocumentAnchorReference(name) | YamlError::ForwardReference(name) => {
            assert_eq!(name, "shared_anchor");
        }
        other => panic!("Expected cross-document anchor rejection, got {:?}", other),
    }
}

#[test]
fn test_12d_yaml_unconsumed_trailing_junk_fails_closed() {
    // Valid mapping followed by unconsumed junk line
    let with_junk = "name: Lust\nage: 3\n--- unconsumed extra document without multi-doc parse";
    let res = yaml::from_str(with_junk);
    assert!(
        res.is_err(),
        "Single-document parse must reject unconsumed trailing junk: got {:?}",
        res
    );
    match res.unwrap_err() {
        YamlError::UnconsumedTrailingJunk(_) | YamlError::SyntaxError(_) => {}
        other => panic!("Expected unconsumed trailing junk error, got {:?}", other),
    }
}

#[test]
fn test_12d_yaml_cyclic_reference_fails_closed() {
    let cyclic = "a: &loop\n  b: *loop";
    let mut parser = LustSafeYaml12::new().with_opt_in_aliases();
    let res = parser.parse(cyclic);
    assert!(res.is_err());
}

// ============================================================================
// 3. @std/toon: Compact Row/Column Frontend & Exact Integers (12A Rule)
// ============================================================================

#[test]
fn test_12d_toon_table_row_and_column_frontend() {
    let columns = vec!["id".to_string(), "name".to_string(), "score".to_string()];
    let cols_data = vec![
        vec![DataValue::from(1), DataValue::from(2), DataValue::from(3)],
        vec![DataValue::from("Alice"), DataValue::from("Bob"), DataValue::from("Carol")],
        vec![DataValue::from(90.5), DataValue::from(85.0), DataValue::from(99.0)],
    ];

    // Columnar construction
    let table = ToonTable::from_columns(columns.clone(), cols_data).expect("valid table from columns");
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.column_count(), 3);
    assert_eq!(table.columns(), &["id", "name", "score"]);

    // Cell lookup
    assert_eq!(table.get_cell(0, "name"), Some(&DataValue::String("Alice".to_string())));
    assert_eq!(table.get_cell(1, "id"), Some(&DataValue::Number(ExactNumber::Int(2))));
    assert_eq!(table.get_cell(2, "score"), Some(&DataValue::Number(ExactNumber::Float(99.0))));

    // Columnar slice lookup
    let names = table.column("name").expect("column values");
    assert_eq!(names, vec![
        &DataValue::String("Alice".to_string()),
        &DataValue::String("Bob".to_string()),
        &DataValue::String("Carol".to_string()),
    ]);

    // Encode to compact string and decode back
    let encoded = toon::encode_table(&table).expect("compact encoding");
    let decoded_table = toon::decode_table(&encoded).expect("decoded table");
    assert_eq!(table, decoded_table);
}

#[test]
fn test_12d_toon_exact_integers_12a_rule_rejection() {
    // Exact integers must succeed
    let valid_int = "val\n9223372036854775807"; // i64::MAX
    let val = toon::from_str(valid_int).unwrap();
    if let DataValue::Array(rows) = val {
        if let DataValue::Document(d) = &rows[0] {
            assert_eq!(d.get("val").unwrap(), &DataValue::Number(ExactNumber::Int(i64::MAX)));
        }
    }

    let valid_neg = "val\n-9223372036854775808"; // i64::MIN
    let val_neg = toon::from_str(valid_neg).unwrap();
    if let DataValue::Array(rows) = val_neg {
        if let DataValue::Document(d) = &rows[0] {
            assert_eq!(d.get("val").unwrap(), &DataValue::Number(ExactNumber::Int(i64::MIN)));
        }
    }

    // Integer token exceeding i64 lane must reject and NOT narrow to f64
    let overflow_pos = "val\n9223372036854775808"; // i64::MAX + 1
    assert!(toon::from_str(overflow_pos).is_err(), "integer token overflow must reject instead of silent f64 narrowing");

    let overflow_large = "val\n18446744073709551616"; // 2^64
    assert!(toon::from_str(overflow_large).is_err(), "integer token overflow must reject instead of silent f64 narrowing");
}

// ============================================================================
// 4. @std/... Module Discovery and Virtual Std Registry
// ============================================================================

#[test]
fn test_12d_std_module_registration_and_permission() {
    let config = LustConfig::default();
    assert!(config.is_std_module_allowed("@std/bson"));
    assert!(config.is_std_module_allowed("@std/yaml"));
    assert!(config.is_std_module_allowed("@std/toon"));

    assert!(is_virtual_std_module("@std/bson"));
    assert!(is_virtual_std_module("@std/yaml"));
    assert!(is_virtual_std_module("@std/toon"));

    assert_eq!(canonical_std_module_name("@std/bson"), "bson");
    assert_eq!(canonical_std_module_name("@std/yaml"), "yaml");
    assert_eq!(canonical_std_module_name("@std/toon"), "toon");

    // Symbol resolution
    assert_eq!(resolve_std_function("@std/bson", "from_slice"), Some("bson.from_slice"));
    assert_eq!(resolve_std_function("@std/bson", "view"), Some("bson.view"));
    assert_eq!(resolve_std_function("@std/yaml", "from_str"), Some("yaml.from_str"));
    assert_eq!(resolve_std_function("@std/yaml", "parse_documents"), Some("yaml.parse_documents"));
    assert_eq!(resolve_std_function("@std/toon", "from_str"), Some("toon.from_str"));
    assert_eq!(resolve_std_function("@std/toon", "decode_table"), Some("toon.decode_table"));
}
