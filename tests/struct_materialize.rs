use lust::ast::{FieldOwnership, Item, ItemKind, Span, StructDef, StructField, Type, TypeKind, Visibility};
use lust::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
use lust::data::materialize::{
    materialize_from_document, materialize_from_item, materialize_into_struct, MaterializeError,
};
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

#[test]
fn test_materialize_null_where_not_nullable_fails_closed() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64).with_nullability(false, false),
    ];
    let layout = LogicalLayoutV1::new_with_abi(2004, "StrictNotNull", fields, TargetAbi::x86_64());

    let mut doc = Document::new();
    doc.try_insert("id".to_string(), DataValue::Null).unwrap();

    let res = materialize_from_document(&layout, &doc);
    assert!(
        matches!(res, Err(MaterializeError::NullWhereNotNullable { ordinal: 0, .. })),
        "Null value on a non-nullable field must fail closed with NullWhereNotNullable"
    );
}

#[test]
fn test_materialize_nominal_type_mismatch_fails_closed() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
    ];
    let layout = LogicalLayoutV1::new_with_abi(2005, "User", fields, TargetAbi::x86_64());

    let mut doc = Document::new();
    doc.try_insert("_type".to_string(), DataValue::String("Product".to_string())).unwrap();
    doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(1))).unwrap();

    let res = materialize_from_document(&layout, &doc);
    assert!(
        matches!(
            res,
            Err(MaterializeError::NominalTypeMismatch {
                ref expected,
                ref received
            }) if expected == "User" && received == "Product"
        ),
        "Nominal type mismatch must fail closed"
    );
}

#[test]
fn test_materialize_required_nullable_absent_is_missing_field_not_null() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "nickname", CanonicalTypeTag::StringSlice).with_nullability(true, false),
    ];
    let layout = LogicalLayoutV1::new_with_abi(2006, "Member", fields, TargetAbi::x86_64());

    let mut doc = Document::new();
    doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(100))).unwrap();

    // Required nullable field is absent -> must fail with MissingField, NOT default to null
    let res = materialize_from_document(&layout, &doc);
    assert!(
        matches!(res, Err(MaterializeError::MissingField { ordinal: 1, .. })),
        "A required nullable field that is absent must be MissingField, not null"
    );

    // Explicit null provided -> succeeds
    doc.try_insert("nickname".to_string(), DataValue::Null).unwrap();
    let mat = materialize_from_document(&layout, &doc).unwrap();
    assert_eq!(mat.get_slot(1).unwrap(), &DataValue::Null);
}

#[test]
fn test_direct_materialization_into_item_kind_struct_slots() {
    let def = StructDef {
        name: "InventoryItem".to_string(),
        type_params: Vec::new(),
        trait_bounds: Vec::new(),
        fields: vec![
            StructField {
                name: "sku".to_string(),
                ty: Type::new(TypeKind::String, Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
            StructField {
                name: "quantity".to_string(),
                ty: Type::new(TypeKind::Int, Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
        ],
        visibility: Visibility::Public,
        doc: None,
    };

    let mut doc = Document::new();
    doc.try_insert("quantity".to_string(), DataValue::Number(ExactNumber::Int(50))).unwrap();
    doc.try_insert("sku".to_string(), DataValue::String("WIDGET-01".to_string())).unwrap();

    let mat = materialize_into_struct(&def, &doc).unwrap();
    assert_eq!(mat.get_slot(0).unwrap(), &DataValue::String("WIDGET-01".to_string()));
    assert_eq!(mat.get_slot(1).unwrap(), &DataValue::Number(ExactNumber::Int(50)));

    // Test via ItemKind::Struct
    let item = Item::new(ItemKind::Struct(def), Span::dummy());
    let mat_item = materialize_from_item(&item, &doc).unwrap();
    assert_eq!(mat, mat_item);
}

#[test]
fn test_materialized_struct_conversions_bson_json_db_params() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "title", CanonicalTypeTag::StringSlice),
    ];
    let layout = LogicalLayoutV1::new_with_abi(2007, "Post", fields, TargetAbi::x86_64());

    let mut doc = Document::new();
    doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(7))).unwrap();
    doc.try_insert("title".to_string(), DataValue::String("Hello Lust".to_string())).unwrap();

    let mat = materialize_from_document(&layout, &doc).unwrap();

    // to_bson
    let bson_bytes = mat.to_bson();
    assert!(!bson_bytes.is_empty());
    let decoded_doc = lust::data::bson::from_slice(&bson_bytes).unwrap();
    assert_eq!(decoded_doc.get("id"), Some(&DataValue::Number(ExactNumber::Int(7))));

    // to_json
    let json_str = mat.to_json().unwrap();
    assert!(json_str.contains("\"title\":\"Hello Lust\""));

    // to_db_params
    let params = mat.to_db_params();
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].0, "id");
    assert_eq!(params[0].1, DataValue::Number(ExactNumber::Int(7)));
    assert_eq!(params[1].0, "title");
    assert_eq!(params[1].1, DataValue::String("Hello Lust".to_string()));
}
