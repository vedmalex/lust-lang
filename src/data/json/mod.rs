pub mod encode;
pub mod events;
pub mod scalar;

pub use encode::{to_string, to_string_pretty};
pub use events::{JsonEvent, JsonError};
pub use scalar::JsonParser;

use crate::data::DataValue;

/// Parse a complete JSON string into a `DataValue` using strict RFC 8259 rules.
/// Duplicate keys are strictly rejected (Gate-SCHEMA-01).
pub fn from_str(s: &str) -> Result<DataValue, JsonError> {
    let mut parser = JsonParser::new(s.as_bytes());
    parser.parse_all()
}

/// Parse JSON bytes into a `DataValue` with custom nesting depth and byte limits.
pub fn from_slice_with_limits(
    bytes: &[u8],
    max_depth: usize,
    max_bytes: usize,
) -> Result<DataValue, JsonError> {
    let mut parser = JsonParser::with_limits(bytes, max_depth, max_bytes);
    parser.parse_all()
}
