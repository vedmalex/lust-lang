use lust::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
use lust::data::materialize::materialize_from_document;
use lust::data::{DataValue, Document, ExactNumber};
use lust::data::materialize::MaterializeError;
use lust::regex::BoundedRegex;
use lust::schema::s;
use lust::schema::binding::StructSchemaBinding;

#[test]
fn review_gate_schema_08_malformed_regex_must_not_validate() {
    let schema = s.string().regex("[").build();
    let value = DataValue::String("ordinary text".to_string());

    assert!(
        schema.parse(&value).is_err(),
        "invalid regex must fail closed instead of being ignored"
    );
}

#[test]
fn review_gate_schema_04_find_scans_no_match_input_linearly() {
    let regex = BoundedRegex::new("a*z").unwrap();
    let long = "a".repeat(20_000);

    let (found, stats) = regex.find_with_stats(&long).unwrap();
    assert_eq!(found, None);
    assert_eq!(
        stats.dfa_searches, 1,
        "a no-match find is one forward DFA search, not one search per byte"
    );
}

#[test]
fn review_gate_schema_08_find_scans_matched_pattern_linearly() {
    let regex = BoundedRegex::new("a+b").unwrap();
    let n1 = 10_000;
    let short = format!("{}c{}b", "a".repeat(n1), "a".repeat(n1));
    let n2 = 20_000;
    let long = format!("{}c{}b", "a".repeat(n2), "a".repeat(n2));

    let (m_short, short_stats) = regex.find_with_stats(&short).unwrap();
    let m_short = m_short.expect("must find match");

    assert_eq!(m_short.start, n1 + 1);
    assert_eq!(m_short.end, 2 * n1 + 2);
    assert_eq!(m_short.text, format!("{}b", "a".repeat(n1)));
    assert_eq!(short_stats.dfa_searches, 2);

    let (m_long, long_stats) = regex.find_with_stats(&long).unwrap();
    let m_long = m_long.expect("must find match");

    assert_eq!(m_long.start, n2 + 1);
    assert_eq!(m_long.end, 2 * n2 + 2);
    assert_eq!(m_long.text, format!("{}b", "a".repeat(n2)));
    assert_eq!(
        long_stats.dfa_searches, 2,
        "a match is one forward search plus one reverse search, independent of input length"
    );
}

#[test]
fn review_gate_schema_09_layout_ordinals_are_contiguous_and_unique() {
    let fields = vec![
        LogicalFieldDesc::new(1, "first", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(2, "second", CanonicalTypeTag::Int64),
    ];
    let layout = LogicalLayoutV1::new_with_abi(9901, "BadOrdinals", fields, TargetAbi::x86_64());

    let mut document = Document::new();
    document
        .try_insert("first".to_string(), DataValue::Number(ExactNumber::Int(11)))
        .unwrap();
    document
        .try_insert("second".to_string(), DataValue::Number(ExactNumber::Int(22)))
        .unwrap();
    let materialized = materialize_from_document(&layout, &document).unwrap();

    for field in &layout.fields {
        let by_name = document.get(&field.name).unwrap();
        assert_eq!(materialized.get_slot(field.ordinal as usize), Some(by_name));
    }
}

#[test]
fn review_gate_schema_09_missing_nullable_required_field_is_rejected() {
    let fields = vec![
        LogicalFieldDesc::new(0, "required_nullable", CanonicalTypeTag::Int64)
            .with_nullability(true, false),
    ];
    let layout = LogicalLayoutV1::new_with_abi(9902, "NullableRequired", fields, TargetAbi::x86_64());
    let materialized = materialize_from_document(&layout, &Document::new());

    assert!(
        matches!(materialized, Err(MaterializeError::MissingField { ordinal: 0, .. })),
        "nullable means explicit null is permitted; only optional fields may be omitted"
    );
}

#[test]
fn review_gate_schema_08_binary_layout_binding_rejects_string_value() {
    let layout = LogicalLayoutV1::new_with_abi(
        9903,
        "BinaryContainer",
        vec![LogicalFieldDesc::new(0, "payload", CanonicalTypeTag::BinarySlice)],
        TargetAbi::x86_64(),
    );
    let binding = StructSchemaBinding::new(layout);
    let mut document = Document::new();
    document
        .try_insert("payload".to_string(), DataValue::String("not binary".to_string()))
        .unwrap();

    assert!(
        binding.schema.parse(&DataValue::Document(document)).is_err(),
        "layout-derived schema must preserve BinarySlice instead of mapping it to Any"
    );
}
