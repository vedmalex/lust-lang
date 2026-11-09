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
    let short = "a".repeat(10_000);
    let long = "a".repeat(20_000);

    let start = std::time::Instant::now();
    assert_eq!(regex.find(&short).unwrap(), None);
    let short_elapsed = start.elapsed();

    let start = std::time::Instant::now();
    assert_eq!(regex.find(&long).unwrap(), None);
    let long_elapsed = start.elapsed();

    assert!(
        long_elapsed.as_nanos() <= short_elapsed.as_nanos().saturating_mul(3).max(5_000_000),
        "doubling input should not approach quadratic search time: short={short_elapsed:?}, long={long_elapsed:?}"
    );
}

#[test]
fn review_gate_schema_08_find_scans_matched_pattern_linearly() {
    let regex = BoundedRegex::new("a+b").unwrap();
    let n1 = 10_000;
    let short = format!("{}c{}b", "a".repeat(n1), "a".repeat(n1));
    let n2 = 20_000;
    let long = format!("{}c{}b", "a".repeat(n2), "a".repeat(n2));

    let start = std::time::Instant::now();
    let m_short = regex.find(&short).unwrap().expect("must find match");
    let short_elapsed = start.elapsed();

    assert_eq!(m_short.start, n1 + 1);
    assert_eq!(m_short.end, 2 * n1 + 2);
    assert_eq!(m_short.text, format!("{}b", "a".repeat(n1)));

    let start = std::time::Instant::now();
    let m_long = regex.find(&long).unwrap().expect("must find match");
    let long_elapsed = start.elapsed();

    assert_eq!(m_long.start, n2 + 1);
    assert_eq!(m_long.end, 2 * n2 + 2);
    assert_eq!(m_long.text, format!("{}b", "a".repeat(n2)));

    // Under quadratic repeated scans, 20,002 bytes took 1.7s and 40,002 bytes took 6.7s.
    // Under linear reverse DFA search, 40,002 bytes finishes in a few milliseconds.
    assert!(
        long_elapsed.as_millis() < 300,
        "find must scale linearly and not repeat suffix scan quadratically: took {long_elapsed:?}"
    );
    assert!(
        long_elapsed.as_nanos() <= short_elapsed.as_nanos().saturating_mul(4).max(10_000_000),
        "doubling input should scale linearly: short={short_elapsed:?}, long={long_elapsed:?}"
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
