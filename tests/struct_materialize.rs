use lust::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
use lust::data::materialize::{materialize_from_document, MaterializeError};
use lust::data::{DataValue, Document, ExactNumber};

#[test]
fn test_gate_09_direct_vm_slot_materialization_by_ordinal() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "email", CanonicalTypeTag::StringSlice),
        LogicalFieldDesc::new(2, "balance", CanonicalTypeTag::Float64),
    ];

    let layout = LogicalLayoutV1::new_with_abi(2001, "Account", fields, TargetAbi::x86_64());

    let mut doc = Document::new();
    doc.try_insert("balance".to_string(), DataValue::Number(ExactNumber::Float(1500.50))).unwrap();
    doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(42))).unwrap();
    doc.try_insert("email".to_string(), DataValue::String("test@lust-lang.dev".to_string())).unwrap();

    let mat = materialize_from_document(&layout, &doc).unwrap();

    // Verify slots are ordered strictly by ordinal (0..N-1), not by wire order in doc
    assert_eq!(mat.get_slot(0).unwrap(), &DataValue::Number(ExactNumber::Int(42)));
    assert_eq!(mat.get_slot(1).unwrap(), &DataValue::String("test@lust-lang.dev".to_string()));
    assert_eq!(mat.get_slot(2).unwrap(), &DataValue::Number(ExactNumber::Float(1500.50)));

    // Verify roundtrip to Document preserves field names in declaration order
    let demat_doc = mat.to_document();
    let entries: Vec<(String, DataValue)> = demat_doc.into_iter().collect();
    assert_eq!(entries[0].0, "id");
    assert_eq!(entries[1].0, "email");
    assert_eq!(entries[2].0, "balance");
}

#[test]
fn test_materialize_type_mismatch_fails_closed() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
    ];
    let layout = LogicalLayoutV1::new_with_abi(2002, "Simple", fields, TargetAbi::x86_64());

    let mut doc = Document::new();
    doc.try_insert("id".to_string(), DataValue::String("not_an_int".to_string())).unwrap();

    let res = materialize_from_document(&layout, &doc);
    assert!(matches!(res, Err(MaterializeError::TypeMismatch { ordinal: 0, .. })));
}

#[test]
fn test_materialize_missing_required_field_fails_closed() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "name", CanonicalTypeTag::StringSlice),
    ];
    let layout = LogicalLayoutV1::new_with_abi(2003, "Person", fields, TargetAbi::x86_64());

    let mut doc = Document::new();
    doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(10))).unwrap();

    let res = materialize_from_document(&layout, &doc);
    assert!(matches!(res, Err(MaterializeError::MissingField { ordinal: 1, .. })));
}
