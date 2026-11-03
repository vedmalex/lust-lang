// Isolated test probe for Issue #11 Gate 1 follow-up:
// Empirically evaluating pinned serde_json 1.0.151 with `arbitrary_precision`.

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde_json::Number;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
enum RawCallback {
    I64(i64),
    U64(u64),
    F64(f64),
    MapPrivateNumber(String),
    Other(String),
}

struct AnyProbeVisitor;

impl<'de> Visitor<'de> for AnyProbeVisitor {
    type Value = RawCallback;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any JSON scalar or number-map")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(RawCallback::I64(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(RawCallback::U64(v))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(RawCallback::F64(v))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        // In arbitrary_precision, numbers that don't fit u64/i64 or are floats dispatch as
        // a synthetic private map: {"$serde_json::private::Number": "<raw_text>"}
        let mut desc = String::new();
        while let Some(key) = map.next_key::<String>()? {
            let val = map.next_value::<String>()?;
            desc = format!("{}:{}", key, val);
        }
        Ok(RawCallback::MapPrivateNumber(desc))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(RawCallback::Other(format!("str:{}", v)))
    }
}

fn probe_any(json: &str) -> Result<RawCallback, serde_json::Error> {
    let mut de = serde_json::Deserializer::from_str(json);
    de.deserialize_any(AnyProbeVisitor)
}

fn probe_number(json: &str) -> Result<Number, serde_json::Error> {
    serde_json::from_str::<Number>(json)
}

#[test]
fn test_arbitrary_precision_number_behavior() {
    println!("\n=== EMPIRICAL TEST: serde_json 1.0.151 with arbitrary_precision ===");

    let cases = [
        "-0",
        "-0.0",
        "9223372036854775807",   // i64::MAX
        "-9223372036854775808",  // i64::MIN
        "9223372036854775808",   // i64::MAX + 1 (u64 only)
        "-9223372036854775809",  // i64::MIN - 1 (out of range int)
        "18446744073709551615",  // u64::MAX
        "18446744073709551616",  // u64::MAX + 1 (overflow int)
        "1.0",
        "1e0",
        "1e-500",
        "1e500",
        "0",
        "1",
        "-1",
    ];

    for c in cases {
        let any_res = probe_any(c);
        let num_res = probe_number(c);

        println!("--------------------------------------------------");
        println!("Input: {:?}", c);
        println!("  deserialize_any callback: {:?}", any_res);
        match &num_res {
            Ok(num) => {
                println!("  Number deserialization: OK");
                println!("    as_str(): {:?}", num.as_str());
                println!("    as_i64(): {:?}", num.as_i64());
                println!("    as_u64(): {:?}", num.as_u64());
                println!("    as_f64(): {:?}", num.as_f64());
                println!("    is_i64(): {:?}", num.is_i64());
                println!("    is_u64(): {:?}", num.is_u64());
                println!("    is_f64(): {:?}", num.is_f64());
            }
            Err(e) => {
                println!("  Number deserialization: ERR({:?})", e.to_string());
            }
        }
    }
}

#[test]
fn test_specific_empirical_assertions() {
    // 1. Negative integer zero: "-0"
    // Fact: deserialize_any routes to visit_i64(0) because "-0".parse::<i64>() == Ok(0).
    // Fact: Number::deserialize routes to visit_i64(0) and formats it as "0".
    // Therefore, the negative sign is LOST on "-0" in both paths!
    let any_neg_zero = probe_any("-0").unwrap();
    assert_eq!(any_neg_zero, RawCallback::I64(0));
    let num_neg_zero = probe_number("-0").unwrap();
    assert_eq!(num_neg_zero.as_str(), "0", "'-0' loses sign and becomes '0'");
    assert_eq!(num_neg_zero.as_i64(), Some(0));
    assert_eq!(num_neg_zero.as_f64(), Some(0.0));

    // 2. Negative float zero: "-0.0"
    // Fact: "-0.0" cannot parse as i64, so it falls back to String("-0.0").
    // deserialize_any routes to MapPrivateNumber.
    // Number::as_str() retains "-0.0".
    let any_neg_float_zero = probe_any("-0.0").unwrap();
    assert_eq!(
        any_neg_float_zero,
        RawCallback::MapPrivateNumber("$serde_json::private::Number:-0.0".to_string())
    );
    let num_neg_float_zero = probe_number("-0.0").unwrap();
    assert_eq!(num_neg_float_zero.as_str(), "-0.0");
    assert_eq!(num_neg_float_zero.as_i64(), None);
    assert_eq!(num_neg_float_zero.as_f64(), Some(-0.0));
    assert!(num_neg_float_zero.as_f64().unwrap().is_sign_negative());

    // 3. i64::MAX and i64::MIN
    let num_i64_max = probe_number("9223372036854775807").unwrap();
    assert_eq!(num_i64_max.as_i64(), Some(i64::MAX));
    assert_eq!(num_i64_max.as_str(), "9223372036854775807");

    let num_i64_min = probe_number("-9223372036854775808").unwrap();
    assert_eq!(num_i64_min.as_i64(), Some(i64::MIN));
    assert_eq!(num_i64_min.as_str(), "-9223372036854775808");

    // 4. Out-of-range negative integer: "-9223372036854775809" (< i64::MIN)
    // Fact: Does not fit i64. parse_any_number returns String.
    // Number retains full raw string "-9223372036854775809".
    // as_i64() is None. as_f64() parses to float (with precision loss).
    let num_i64_underflow = probe_number("-9223372036854775809").unwrap();
    assert_eq!(num_i64_underflow.as_str(), "-9223372036854775809");
    assert_eq!(num_i64_underflow.as_i64(), None);
    assert!(num_i64_underflow.as_f64().is_some());

    // 5. Out-of-range positive integers:
    // a) 9223372036854775808 (i64::MAX + 1, fits in u64)
    // Fact: deserialize_any routes to visit_u64(9223372036854775808).
    // Number::as_i64() is None, as_u64() is Some(9223372036854775808).
    let num_u64_only = probe_number("9223372036854775808").unwrap();
    assert_eq!(num_u64_only.as_i64(), None);
    assert_eq!(num_u64_only.as_u64(), Some(9223372036854775808));
    assert_eq!(num_u64_only.as_str(), "9223372036854775808");

    // b) 18446744073709551616 (u64::MAX + 1, exceeds u64)
    // Fact: parse_any_number returns String.
    // Number retains full raw string "18446744073709551616".
    // as_i64() is None, as_u64() is None. as_f64() is Some(18446744073709551616.0).
    let num_u64_overflow = probe_number("18446744073709551616").unwrap();
    assert_eq!(num_u64_overflow.as_str(), "18446744073709551616");
    assert_eq!(num_u64_overflow.as_i64(), None);
    assert_eq!(num_u64_overflow.as_u64(), None);
    assert!(num_u64_overflow.as_f64().is_some());

    // 6. Floats: "1.0" and "1e0"
    // Fact: "1.0" cannot parse as integer. Returns String("1.0").
    let num_one_dot_zero = probe_number("1.0").unwrap();
    assert_eq!(num_one_dot_zero.as_str(), "1.0");
    assert_eq!(num_one_dot_zero.as_i64(), None);
    assert_eq!(num_one_dot_zero.as_f64(), Some(1.0));

    // Fact: "1e0" is normalized by serde_json's scan_exponent to "1e+0".
    let num_one_e_zero = probe_number("1e0").unwrap();
    assert_eq!(num_one_e_zero.as_str(), "1e+0", "scan_exponent normalizes '1e0' to '1e+0'");
    assert_eq!(num_one_e_zero.as_i64(), None);
    assert_eq!(num_one_e_zero.as_f64(), Some(1.0));

    // 7. Float underflow: "1e-500"
    // Fact: Deserializes successfully into Number with as_str() == "1e-500".
    // But Number::as_f64() parses via std::str::parse::<f64> which underflows to 0.0!
    let num_underflow = probe_number("1e-500").unwrap();
    assert_eq!(num_underflow.as_str(), "1e-500");
    assert_eq!(num_underflow.as_f64(), Some(0.0), "as_f64() underflows to 0.0");

    // 8. Float overflow: "1e500"
    // Fact: Deserializes successfully into Number with as_str() == "1e+500"!
    // Unlike default serde_json (which errors at parse time with 'number out of range'),
    // arbitrary_precision SUCCEEDS in deserializing "1e500" into Number!
    // But Number::as_f64() evaluates std::str::parse::<f64>() which returns inf,
    // and filter(f64::is_finite) filters it out, returning None!
    let num_overflow = probe_number("1e500").unwrap();
    assert_eq!(num_overflow.as_str(), "1e+500");
    assert_eq!(num_overflow.as_f64(), None, "as_f64() returns None because value is inf");
}
