use crate::data::DataValue;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonEvent {
    StartObject,
    EndObject,
    StartArray,
    EndArray,
    Key(String),
    Scalar(DataValue),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonError {
    UnexpectedEof,
    UnexpectedChar(char, usize),
    DuplicateKey(String, usize),
    MaxDepthExceeded(usize),
    MaxBytesExceeded(usize),
    TrailingBytes(usize),
    TrailingComma(usize),
    InvalidEscape(usize),
    InvalidUnicode(usize),
    InvalidNumber(usize),
    Io(String),
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsonError::UnexpectedEof => write!(f, "Unexpected end of JSON input"),
            JsonError::UnexpectedChar(c, pos) => write!(f, "Unexpected character '{}' at offset {}", c, pos),
            JsonError::DuplicateKey(k, pos) => write!(f, "Duplicate key '{}' rejected at offset {}", k, pos),
            JsonError::MaxDepthExceeded(depth) => write!(f, "Max nesting depth of {} exceeded", depth),
            JsonError::MaxBytesExceeded(bytes) => write!(f, "Max document bytes of {} exceeded", bytes),
            JsonError::TrailingBytes(pos) => write!(f, "Trailing unconsumed bytes at offset {}", pos),
            JsonError::TrailingComma(pos) => write!(f, "Trailing comma forbidden in strict mode at offset {}", pos),
            JsonError::InvalidEscape(pos) => write!(f, "Invalid escape sequence at offset {}", pos),
            JsonError::InvalidUnicode(pos) => write!(f, "Invalid unicode escape at offset {}", pos),
            JsonError::InvalidNumber(pos) => write!(f, "Invalid number syntax at offset {}", pos),
            JsonError::Io(msg) => write!(f, "I/O error: {}", msg),
        }
    }
}

impl std::error::Error for JsonError {}
