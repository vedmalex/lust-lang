use lust::data::json::{self, JsonError};
use lust::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
use lust::data::materialize::{materialize_from_document, MaterializeError};
use lust::data::owner::{LeaseError, OwnerRegistry};
use lust::data::toon::{self, ToonError};
use lust::data::yaml::{self, YamlError};
use lust::data::{BudgetTracker, DataValue, Document, ExactNumber};
use lust::regex::{compile_bounded, RegexError};
use lust::schema::s;

#[test]
fn test_negative_gate_01_all_formats_reject_duplicate_keys() {
    // JSON
    assert!(matches!(
        json::from_str(r#"{"dup": 1, "dup": 2}"#),
        Err(JsonError::DuplicateKey(..))
    ));

    // YAML
    assert!(matches!(
        yaml::from_str("dup: 1\ndup: 2"),
        Err(YamlError::DuplicateKey(..))
    ));

    // TOON
    assert!(matches!(
        toon::from_str("[dup, col, dup]\n1, 2, 3"),
        Err(ToonError::DuplicateColumn(..))
    ));

    // Document
    let mut doc = Document::new();
    doc.try_insert("k".to_string(), DataValue::Number(ExactNumber::Int(1))).unwrap();
    assert!(doc.try_insert("k".to_string(), DataValue::Number(ExactNumber::Int(2))).is_err());
}

#[test]
fn test_negative_gate_02_budget_tracker_quota_enforcement() {
    let mut tracker = BudgetTracker::new(100, 1000, 10);
    assert!(tracker.charge_bytes(99).is_ok()); // quota - 1
    assert!(tracker.charge_bytes(1).is_ok());  // quota
    assert!(tracker.charge_bytes(1).is_err()); // quota + 1 fails closed
}

#[test]
fn test_negative_gate_03_lease_protocol_release_before_join_rejected() {
    let mut reg = OwnerRegistry::new(1);
    let id = reg.allocate(1);
    reg.cancel(id).unwrap();
    // Releasing without acknowledge_join MUST fail with ReleaseBeforeJoin
    assert_eq!(reg.release(id), Err(LeaseError::ReleaseBeforeJoin));
}

#[test]
fn test_negative_gate_04_regex_rejects_backtracking_and_lookaround() {
    // Backtracking pattern
    assert!(matches!(
        compile_bounded(r"(?=lookahead)test"),
        Err(RegexError::UnsupportedPattern(..))
    ));
    assert!(matches!(
        compile_bounded(r"([a-z])\1"),
        Err(RegexError::UnsupportedPattern(..))
    ));
}

#[test]
fn test_negative_gate_05_yaml_billion_laughs_bomb_rejected() {
    let mut bomb = String::from("root: &b a\n");
    for i in 0..25 {
        bomb.push_str(&format!("k{}: *b\n", i));
    }
    let res = yaml::from_str_with_aliases(&bomb);
    assert!(res.is_err());
}

#[test]
fn test_negative_gate_08_schema_validation_type_mismatch() {
    let user_schema = s
        .object()
        .field("id", s.int())
        .field("email", s.string())
        .build();

    let invalid_json = r#"{"id": "not_an_int", "email": 123}"#;
    let parsed = json::from_str(invalid_json).unwrap();
    let res = user_schema.parse(&parsed);
    assert!(res.is_err());
    let errs = res.unwrap_err().errors;
    assert_eq!(errs.len(), 2);
}

#[test]
fn test_negative_gate_09_struct_materialize_missing_field_rejected() {
    let fields = vec![
        LogicalFieldDesc::new(0, "required_id", CanonicalTypeTag::Int64),
    ];
    let layout = LogicalLayoutV1::new_with_abi(5001, "Req", fields, TargetAbi::x86_64());
    let empty_doc = Document::new();

    let res = materialize_from_document(&layout, &empty_doc);
    assert!(matches!(res, Err(MaterializeError::MissingField { .. })));
}
