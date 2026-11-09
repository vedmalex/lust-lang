use lust::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
use lust::data::{DataValue, Document, ExactNumber};
use lust::schema::binding::StructSchemaBinding;

#[test]
fn test_struct_schema_binding_derivation() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "username", CanonicalTypeTag::StringSlice),
        LogicalFieldDesc::new(2, "is_admin", CanonicalTypeTag::Bool),
        LogicalFieldDesc::new(3, "score", CanonicalTypeTag::Float64).with_nullability(true, true),
    ];

    let layout = LogicalLayoutV1::new_with_abi(1001, "User", fields, TargetAbi::x86_64());
    let binding = StructSchemaBinding::new(layout);

    // Verify field access by ordinal
    assert_eq!(binding.get_field_by_ordinal(0).unwrap().name, "id");
    assert_eq!(binding.get_field_by_ordinal(1).unwrap().name, "username");
    assert_eq!(binding.get_field_by_ordinal(2).unwrap().name, "is_admin");
    assert_eq!(binding.get_field_by_ordinal(3).unwrap().name, "score");

    // Validate a valid document against derived schema
    let mut doc = Document::new();
    doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(1))).unwrap();
    doc.try_insert("username".to_string(), DataValue::String("alex".to_string())).unwrap();
    doc.try_insert("is_admin".to_string(), DataValue::Bool(true)).unwrap();
    doc.try_insert("score".to_string(), DataValue::Number(ExactNumber::Float(99.0))).unwrap();

    let doc_val = DataValue::Document(doc);
    assert!(binding.schema.parse(&doc_val).is_ok());

    // Validate document with missing required field fails
    let mut incomplete = Document::new();
    incomplete.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(1))).unwrap();
    let res = binding.schema.parse(&DataValue::Document(incomplete));
    assert!(res.is_err());
}
