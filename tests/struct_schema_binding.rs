use lust::ast::{FieldOwnership, Item, ItemKind, Span, StructDef, StructField, Type, TypeKind, Visibility};
use lust::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
use lust::data::materialize::MaterializeError;
use lust::data::{DataValue, Document, ExactNumber};
use lust::schema::binding::{derive_schema_from_item, StructSchemaBinding};
use lust::schema::s;

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

#[test]
fn test_s_struct_fluent_nominal_builder() {
    let binding = s
        .r#struct("UserProfile")
        .field("id", s.int().min(1))
        .field("handle", s.string().min(3))
        .nullable_field("bio", s.string())
        .optional_field("avatar_url", s.string())
        .build();

    assert_eq!(binding.layout.type_name, "UserProfile");
    assert_eq!(binding.layout.fields.len(), 4);

    // Test valid document with explicit null for required nullable field
    let mut valid_doc = Document::new();
    valid_doc.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(42))).unwrap();
    valid_doc.try_insert("handle".to_string(), DataValue::String("vedmalex".to_string())).unwrap();
    valid_doc.try_insert("bio".to_string(), DataValue::Null).unwrap();

    let mat = binding.materialize(&valid_doc).unwrap();
    assert_eq!(mat.get_slot(0).unwrap(), &DataValue::Number(ExactNumber::Int(42)));
    assert_eq!(mat.get_slot(1).unwrap(), &DataValue::String("vedmalex".to_string()));
    assert_eq!(mat.get_slot(2).unwrap(), &DataValue::Null);
    assert_eq!(mat.get_slot(3).unwrap(), &DataValue::Null); // optional field omitted -> null slot

    // Test missing required nullable field fails closed with MissingField, not null
    let mut missing_nullable = Document::new();
    missing_nullable.try_insert("id".to_string(), DataValue::Number(ExactNumber::Int(42))).unwrap();
    missing_nullable.try_insert("handle".to_string(), DataValue::String("vedmalex".to_string())).unwrap();
    let res = binding.materialize(&missing_nullable);
    assert!(
        matches!(res, Err(MaterializeError::MissingField { ordinal: 2, ref name }) if name == "bio"),
        "A required nullable field that is absent must be MissingField, not null"
    );
}

#[test]
fn test_s_derive_schema_from_ast_struct_def() {
    let def = StructDef {
        name: "Order".to_string(),
        type_params: Vec::new(),
        trait_bounds: Vec::new(),
        fields: vec![
            StructField {
                name: "order_id".to_string(),
                ty: Type::new(TypeKind::Int, Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
            StructField {
                name: "customer".to_string(),
                ty: Type::new(TypeKind::String, Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
            StructField {
                name: "notes".to_string(),
                ty: Type::new(TypeKind::Option(Box::new(Type::new(TypeKind::String, Span::dummy()))), Span::dummy()),
                visibility: Visibility::Public,
                ownership: FieldOwnership::Strong,
                weak_target: None,
            },
        ],
        visibility: Visibility::Public,
        doc: None,
    };

    let schema = s.derive_schema(&def);

    let mut doc = Document::new();
    doc.try_insert("order_id".to_string(), DataValue::Number(ExactNumber::Int(999))).unwrap();
    doc.try_insert("customer".to_string(), DataValue::String("Acme Corp".to_string())).unwrap();

    let res = schema.parse(&DataValue::Document(doc));
    assert!(res.is_ok());

    // Test via item
    let item = Item::new(ItemKind::Struct(def.clone()), Span::dummy());
    let schema_item = derive_schema_from_item(&item).unwrap();
    assert_eq!(schema, schema_item);
}
