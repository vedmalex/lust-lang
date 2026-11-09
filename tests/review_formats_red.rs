//! Independent falsification probes for the frozen Wave 2 format contract.
//! These intentionally assert the contract boundary and may fail against the
//! current implementation; each test exercises a public decoder entry point.

use lust::data::bson;
use lust::data::json::{self, JsonError};
use lust::data::jsonc;
use lust::data::jsonl;
use lust::data::toon;
use lust::data::yaml;

#[test]
fn review_gate01_json_rejects_non_finite_overflow() {
    // JSON has no Infinity value. Parsing an overflowing exponent into f64 must
    // fail closed instead of publishing a non-JSON value.
    assert!(matches!(json::from_str("1e999"), Err(JsonError::InvalidNumber(_))));
}

#[test]
fn review_gate01_toon_rejects_non_finite_overflow() {
    // TOON's scalar parser must not silently turn an invalid numeric scalar
    // into an infinite float (or a string that hides malformed numeric syntax).
    assert!(toon::from_str("value\n1e999").is_err());
}

#[test]
fn review_gate01_yaml_rejects_unconsumed_malformed_suffix() {
    // The public YAML parser must consume the full document. This suffix is
    // not a mapping entry and must not be silently dropped after a valid root.
    assert!(yaml::from_str("a: 1\n- malformed-suffix").is_err());
}

#[test]
fn review_gate01_bson_nested_document_cannot_cross_parent_boundary() {
    // The outer BSON length is 11 bytes, ending immediately after the nested
    // document's 4-byte length field. The embedded document claims 12 bytes and
    // reads through bytes outside the parent's declared region. A trailing
    // zero lets the current whole-buffer decoder finish both reads.
    let bytes = vec![
        11, 0, 0, 0, // outer declared length
        0x03, b'n', 0, // embedded document element and key
        12, 0, 0, 0, // child declared length
        0x10, b'x', 0, 1, 0, 0, 0, // child int32 element
        0, // child terminator
        0, // extra zero consumed as outer terminator
    ];
    assert!(bson::from_slice(&bytes).is_err(), "malformed nested document was accepted: {bytes:?}");
}

#[test]
fn review_gate01_json_rejects_integer_overflow_narrowing() {
    // Integer token 18446744073709551616 (2^64) exceeds integer lanes and must not narrow to f64
    assert!(
        json::from_str("18446744073709551616").is_err(),
        "JSON integer token exceeding integer lanes must be rejected instead of narrowing to f64"
    );
}

#[test]
fn review_gate01_toon_rejects_integer_overflow_narrowing() {
    // Integer token 9223372036854775808 (i64::MAX + 1) exceeds integer lane and must not narrow to f64
    assert!(
        toon::from_str("value\n9223372036854775808").is_err(),
        "TOON integer token exceeding integer lane must be rejected instead of narrowing to f64"
    );
}

#[test]
fn review_gate01_all_json_family_paths_reject_integer_overflow_narrowing() {
    let overflow_token = "18446744073709551616";

    // 1. Scalar JSON
    assert!(
        json::from_str(overflow_token).is_err(),
        "scalar JSON must reject integer token exceeding integer lanes"
    );

    // 2. SIMD JSON
    assert!(
        json::from_str_simd(overflow_token).is_err(),
        "SIMD JSON must reject integer token exceeding integer lanes"
    );

    // 3. JSONC (after comment strip)
    let jsonc_input = format!("// prefix comment\n/* block comment */ {overflow_token}\n");
    assert!(
        jsonc::from_str(&jsonc_input).is_err(),
        "JSONC (after comment strip) must reject integer token exceeding integer lanes"
    );

    // 4. JSONL
    let jsonl_input = format!("{overflow_token}\n");
    let mut reader = jsonl::lines(std::io::Cursor::new(jsonl_input));
    assert!(
        reader.next_value().is_err(),
        "JSONL must reject integer token exceeding integer lanes"
    );
}

