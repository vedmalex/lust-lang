use lust::data::yaml::{self, YamlError};
use lust::data::{DataValue, ExactNumber};

#[test]
fn test_yaml_safe_basic_parsing() {
    let yaml_text = r#"
name: LustSafeYaml12
version: 1
enabled: true
tags:
  - fast
  - safe
"#;
    let res = yaml::from_str(yaml_text).unwrap();
    if let DataValue::Document(doc) = res {
        assert_eq!(doc.get("name").unwrap(), &DataValue::String("LustSafeYaml12".to_string()));
        assert_eq!(doc.get("version").unwrap(), &DataValue::Number(ExactNumber::Int(1)));
        assert_eq!(doc.get("enabled").unwrap(), &DataValue::Bool(true));
    } else {
        panic!("Expected document");
    }
}

#[test]
fn test_yaml_arbitrary_tag_rejected() {
    let malicious = "!ruby/object:Gem::Requirement\nname: test";
    let res = yaml::from_str(malicious);
    assert!(matches!(res, Err(YamlError::UnsupportedTag(_))));
}

#[test]
fn test_yaml_aliases_disabled_by_default() {
    let with_alias = "item: &anchor hello\ncopy: *anchor";
    let res = yaml::from_str(with_alias);
    assert!(res.is_err());
}

#[test]
fn test_gate_05_yaml_billion_laughs_amplification_cap() {
    // 10x expansion is permitted, > 10x expansion is strictly rejected
    // Small base string &a "x" repeated 20 times via alias references
    let mut bomb = String::from("base: &a x\n");
    for i in 0..20 {
        bomb.push_str(&format!("ref{}: *a\n", i));
    }

    let res = yaml::from_str_with_aliases(&bomb);
    assert!(res.is_err());
    match res.unwrap_err() {
        YamlError::AmplificationLimitExceeded { ratio, .. } | YamlError::NodeAmplificationLimitExceeded { ratio, .. } => {
            assert!(ratio > 10.0, "Expected ratio > 10.0, got {}", ratio);
        }
        other => panic!("Expected amplification limit exceeded error, got {:?}", other),
    }
}

#[test]
fn test_yaml_bounded_expansion_accepted() {
    // 3 repetitions of anchor -> ~3x expansion <= 10x cap
    let safe_alias = r#"
base: &a safe_value
copy1: *a
copy2: *a
"#;
    let res = yaml::from_str_with_aliases(safe_alias);
    assert!(res.is_ok());
}

#[test]
fn test_yaml_forward_reference_rejected() {
    let forward_ref = "copy: *future\nbase: &future 42";
    let res = yaml::from_str_with_aliases(forward_ref);
    assert!(matches!(res, Err(YamlError::ForwardReference(_))));
}

#[test]
fn test_gate_01_yaml_duplicate_key_rejected() {
    let duplicate = "id: 1\nid: 2";
    let res = yaml::from_str(duplicate);
    assert!(matches!(res, Err(YamlError::DuplicateKey(_))));
}
