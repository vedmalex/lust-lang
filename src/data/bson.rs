use crate::data::{DataValue, DateTimeUtc, Decimal128, Document, ExactNumber, ObjectId};
use std::fmt;

pub const MAX_BSON_BYTES: usize = 16_777_216; // 16 MiB

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BsonError {
    UnexpectedEof,
    InvalidDocumentLength(i32),
    MaxBytesExceeded(usize),
    MissingNullTerminator,
    InvalidUtf8,
    DuplicateKey(String),
    UnsupportedElementType(u8),
    MalformedSuffix,
    Io(String),
}

impl fmt::Display for BsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BsonError::UnexpectedEof => write!(f, "Unexpected end of BSON data"),
            BsonError::InvalidDocumentLength(len) => write!(f, "Invalid BSON document length header: {}", len),
            BsonError::MaxBytesExceeded(len) => write!(f, "BSON document length {} exceeds maximum {}", len, MAX_BSON_BYTES),
            BsonError::MissingNullTerminator => write!(f, "Missing BSON null terminator"),
            BsonError::InvalidUtf8 => write!(f, "Invalid UTF-8 in BSON string or key"),
            BsonError::DuplicateKey(k) => write!(f, "Duplicate key '{}' rejected in BSON document", k),
            BsonError::UnsupportedElementType(t) => write!(f, "Unsupported BSON element type byte: 0x{:02X}", t),
            BsonError::MalformedSuffix => write!(f, "Trailing unconsumed or malformed bytes in BSON buffer"),
            BsonError::Io(msg) => write!(f, "BSON I/O error: {}", msg),
        }
    }
}

impl std::error::Error for BsonError {}

pub struct BsonDecoder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BsonDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn decode_document(&mut self) -> Result<Document, BsonError> {
        if self.data.len() < 5 {
            return Err(BsonError::UnexpectedEof);
        }
        if self.data.len() > MAX_BSON_BYTES {
            return Err(BsonError::MaxBytesExceeded(self.data.len()));
        }

        let doc_start = self.pos;
        let doc_len = self.read_i32()? as usize;
        if doc_len < 5 || doc_len > (self.data.len() - doc_start) {
            return Err(BsonError::InvalidDocumentLength(doc_len as i32));
        }

        let doc_end = doc_start + doc_len;
        if self.data[doc_end - 1] != 0x00 {
            return Err(BsonError::MissingNullTerminator);
        }

        let mut doc = Document::new();

        while self.pos < doc_end - 1 {
            let elem_type = self.read_u8()?;
            let key = self.read_cstring()?;

            let value = match elem_type {
                0x01 => {
                    // Float 64-bit
                    let f = self.read_f64()?;
                    DataValue::Number(ExactNumber::Float(f))
                }
                0x02 => {
                    // UTF-8 string: int32 length + bytes + 0x00
                    let str_len = self.read_i32()? as usize;
                    if str_len == 0 {
                        return Err(BsonError::UnexpectedEof);
                    }
                    let s_bytes = self.read_slice(str_len - 1)?;
                    self.consume_byte(0x00)?;
                    let s = std::str::from_utf8(s_bytes).map_err(|_| BsonError::InvalidUtf8)?;
                    DataValue::String(s.to_string())
                }
                0x03 => {
                    // Embedded document
                    let sub_doc = self.decode_document()?;
                    DataValue::Document(sub_doc)
                }
                0x04 => {
                    // Array (sub-document with keys "0", "1", ...)
                    let sub_doc = self.decode_document()?;
                    let mut items = Vec::with_capacity(sub_doc.len());
                    for (_, v) in sub_doc.into_iter() {
                        items.push(v);
                    }
                    DataValue::Array(items)
                }
                0x05 => {
                    // Binary: int32 len + subtype (1 byte) + data
                    let bin_len = self.read_i32()? as usize;
                    let _subtype = self.read_u8()?;
                    let bin_bytes = self.read_slice(bin_len)?;
                    DataValue::Binary(bin_bytes.to_vec())
                }
                0x07 => {
                    // ObjectId 12 bytes
                    let raw = self.read_slice(12)?;
                    let mut bytes = [0u8; 12];
                    bytes.copy_from_slice(raw);
                    DataValue::ObjectId(ObjectId(bytes))
                }
                0x08 => {
                    // Boolean 1 byte
                    let b = self.read_u8()?;
                    DataValue::Bool(b != 0)
                }
                0x09 => {
                    // UTC datetime int64
                    let ms = self.read_i64()?;
                    DataValue::DateTimeUtc(DateTimeUtc(ms))
                }
                0x0A => {
                    // Null
                    DataValue::Null
                }
                0x10 => {
                    // 32-bit integer
                    let i = self.read_i32()?;
                    DataValue::Number(ExactNumber::Int(i as i64))
                }
                0x12 => {
                    // 64-bit integer
                    let i = self.read_i64()?;
                    DataValue::Number(ExactNumber::Int(i))
                }
                0x13 => {
                    // 128-bit decimal
                    let raw = self.read_slice(16)?;
                    let mut bytes = [0u8; 16];
                    bytes.copy_from_slice(raw);
                    DataValue::Number(ExactNumber::Decimal128(Decimal128(bytes)))
                }
                other => return Err(BsonError::UnsupportedElementType(other)),
            };

            if let Err(_) = doc.try_insert(key.clone(), value) {
                return Err(BsonError::DuplicateKey(key));
            }
        }

        // Consume doc terminator 0x00
        self.consume_byte(0x00)?;

        Ok(doc)
    }

    fn read_u8(&mut self) -> Result<u8, BsonError> {
        if self.pos >= self.data.len() {
            Err(BsonError::UnexpectedEof)
        } else {
            let b = self.data[self.pos];
            self.pos += 1;
            Ok(b)
        }
    }

    fn read_slice(&mut self, len: usize) -> Result<&'a [u8], BsonError> {
        if self.pos + len > self.data.len() {
            Err(BsonError::UnexpectedEof)
        } else {
            let slice = &self.data[self.pos..self.pos + len];
            self.pos += len;
            Ok(slice)
        }
    }

    fn consume_byte(&mut self, expected: u8) -> Result<(), BsonError> {
        let b = self.read_u8()?;
        if b != expected {
            Err(BsonError::MissingNullTerminator)
        } else {
            Ok(())
        }
    }

    fn read_i32(&mut self) -> Result<i32, BsonError> {
        let slice = self.read_slice(4)?;
        Ok(i32::from_le_bytes(slice.try_into().unwrap()))
    }

    fn read_i64(&mut self) -> Result<i64, BsonError> {
        let slice = self.read_slice(8)?;
        Ok(i64::from_le_bytes(slice.try_into().unwrap()))
    }

    fn read_f64(&mut self) -> Result<f64, BsonError> {
        let slice = self.read_slice(8)?;
        Ok(f64::from_le_bytes(slice.try_into().unwrap()))
    }

    fn read_cstring(&mut self) -> Result<String, BsonError> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0x00 {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(BsonError::MissingNullTerminator);
        }
        let s_bytes = &self.data[start..self.pos];
        self.pos += 1; // skip null byte
        let s = std::str::from_utf8(s_bytes).map_err(|_| BsonError::InvalidUtf8)?;
        Ok(s.to_string())
    }
}

pub struct BsonEncoder {
    buf: Vec<u8>,
}

impl BsonEncoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn encode_document(&mut self, doc: &Document) -> Vec<u8> {
        let start_pos = self.buf.len();
        // Reserve 4 bytes for total document length
        self.buf.extend_from_slice(&[0u8; 4]);

        for (key, val) in doc.iter() {
            self.encode_element(key, val);
        }

        // Document terminator
        self.buf.push(0x00);

        let total_len = (self.buf.len() - start_pos) as i32;
        self.buf[start_pos..start_pos + 4].copy_from_slice(&total_len.to_le_bytes());

        self.buf.clone()
    }

    fn encode_element(&mut self, key: &str, val: &DataValue) {
        match val {
            DataValue::Null => {
                self.buf.push(0x0A);
                self.write_cstring(key);
            }
            DataValue::Bool(b) => {
                self.buf.push(0x08);
                self.write_cstring(key);
                self.buf.push(if *b { 0x01 } else { 0x00 });
            }
            DataValue::Number(ExactNumber::Int(i)) => {
                if *i >= i32::MIN as i64 && *i <= i32::MAX as i64 {
                    self.buf.push(0x10);
                    self.write_cstring(key);
                    self.buf.extend_from_slice(&(*i as i32).to_le_bytes());
                } else {
                    self.buf.push(0x12);
                    self.write_cstring(key);
                    self.buf.extend_from_slice(&i.to_le_bytes());
                }
            }
            DataValue::Number(ExactNumber::UInt(u)) => {
                self.buf.push(0x12);
                self.write_cstring(key);
                self.buf.extend_from_slice(&(*u as i64).to_le_bytes());
            }
            DataValue::Number(ExactNumber::Float(f)) => {
                self.buf.push(0x01);
                self.write_cstring(key);
                self.buf.extend_from_slice(&f.to_le_bytes());
            }
            DataValue::Number(ExactNumber::Decimal128(d)) => {
                self.buf.push(0x13);
                self.write_cstring(key);
                self.buf.extend_from_slice(&d.0);
            }
            DataValue::String(s) => {
                self.buf.push(0x02);
                self.write_cstring(key);
                let str_len = (s.len() + 1) as i32;
                self.buf.extend_from_slice(&str_len.to_le_bytes());
                self.buf.extend_from_slice(s.as_bytes());
                self.buf.push(0x00);
            }
            DataValue::Document(sub_doc) => {
                self.buf.push(0x03);
                self.write_cstring(key);
                let mut sub_encoder = BsonEncoder::new();
                let sub_bytes = sub_encoder.encode_document(sub_doc);
                self.buf.extend_from_slice(&sub_bytes);
            }
            DataValue::Array(items) => {
                self.buf.push(0x04);
                self.write_cstring(key);
                let mut arr_doc = Document::new();
                for (idx, item) in items.iter().enumerate() {
                    let _ = arr_doc.try_insert(idx.to_string(), item.clone());
                }
                let mut sub_encoder = BsonEncoder::new();
                let sub_bytes = sub_encoder.encode_document(&arr_doc);
                self.buf.extend_from_slice(&sub_bytes);
            }
            DataValue::Binary(bytes) => {
                self.buf.push(0x05);
                self.write_cstring(key);
                self.buf.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
                self.buf.push(0x00); // generic binary subtype
                self.buf.extend_from_slice(bytes);
            }
            DataValue::ObjectId(oid) => {
                self.buf.push(0x07);
                self.write_cstring(key);
                self.buf.extend_from_slice(&oid.0);
            }
            DataValue::DateTimeUtc(dt) => {
                self.buf.push(0x09);
                self.write_cstring(key);
                self.buf.extend_from_slice(&dt.0.to_le_bytes());
            }
        }
    }

    fn write_cstring(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0x00);
    }
}

pub fn from_slice(bytes: &[u8]) -> Result<Document, BsonError> {
    let mut decoder = BsonDecoder::new(bytes);
    let doc = decoder.decode_document()?;
    if decoder.pos < bytes.len() {
        return Err(BsonError::MalformedSuffix);
    }
    Ok(doc)
}

pub fn to_vec(doc: &Document) -> Vec<u8> {
    let mut encoder = BsonEncoder::new();
    encoder.encode_document(doc)
}
