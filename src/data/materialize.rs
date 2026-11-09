use crate::data::layout::{CanonicalTypeTag, LogicalLayoutV1};
use crate::data::{DataValue, Document, ExactNumber};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum MaterializeError {
    MissingField { ordinal: u16, name: String },
    TypeMismatch { ordinal: u16, name: String, expected: CanonicalTypeTag },
    ExtraFieldInStrictLayout(String),
}

impl fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaterializeError::MissingField { ordinal, name } => {
                write!(f, "Missing required field '{}' at ordinal {}", name, ordinal)
            }
            MaterializeError::TypeMismatch { ordinal, name, expected } => {
                write!(f, "Type mismatch for field '{}' at ordinal {}: expected {:?}", name, ordinal, expected)
            }
            MaterializeError::ExtraFieldInStrictLayout(k) => {
                write!(f, "Unrecognized field '{}' in strict struct layout", k)
            }
        }
    }
}

impl std::error::Error for MaterializeError {}

/// Materialized struct representation with fixed slots indexed directly by declaration ordinal.
/// Conforms to Gate-SCHEMA-09: Direct VM slot decoding decodes fields by ordinal without hash lookups.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializedStruct {
    pub layout: LogicalLayoutV1,
    pub slots: Vec<DataValue>, // strictly indexed by field.ordinal (0..N-1)
}

impl MaterializedStruct {
    pub fn new(layout: LogicalLayoutV1, slots: Vec<DataValue>) -> Self {
        Self { layout, slots }
    }

    /// O(1) direct slot read by declaration-order ordinal (Gate-SCHEMA-09)
    #[inline(always)]
    pub fn get_slot(&self, ordinal: usize) -> Option<&DataValue> {
        self.slots.get(ordinal)
    }

    /// O(1) direct slot write by declaration-order ordinal
    #[inline(always)]
    pub fn set_slot(&mut self, ordinal: usize, val: DataValue) -> Result<(), MaterializeError> {
        if ordinal < self.slots.len() {
            self.slots[ordinal] = val;
            Ok(())
        } else {
            Err(MaterializeError::MissingField {
                ordinal: ordinal as u16,
                name: format!("slot_{}", ordinal),
            })
        }
    }

    /// Dematerialize back into an ordered Document
    pub fn to_document(&self) -> Document {
        let mut doc = Document::with_capacity(self.layout.fields.len());
        for field in &self.layout.fields {
            let val = self.slots.get(field.ordinal as usize).cloned().unwrap_or(DataValue::Null);
            let _ = doc.try_insert(field.name.clone(), val);
        }
        doc
    }
}

/// Materialize a Document into fixed slots according to declaration-order ordinals.
/// Rejects partial initialization on failure and cleans up in reverse order.
pub fn materialize_from_document(
    layout: &LogicalLayoutV1,
    doc: &Document,
) -> Result<MaterializedStruct, MaterializeError> {
    let mut slots = Vec::with_capacity(layout.fields.len());

    for field in &layout.fields {
        match doc.get(&field.name) {
            Some(val) => {
                if !matches_tag(val, field.tag) {
                    if field.is_nullable && matches!(val, DataValue::Null) {
                        slots.push(DataValue::Null);
                    } else {
                        // Cleanup allocated slots on failure
                        slots.clear();
                        return Err(MaterializeError::TypeMismatch {
                            ordinal: field.ordinal,
                            name: field.name.clone(),
                            expected: field.tag,
                        });
                    }
                } else {
                    slots.push(val.clone());
                }
            }
            None => {
                if field.is_optional || field.is_nullable {
                    slots.push(DataValue::Null);
                } else {
                    slots.clear();
                    return Err(MaterializeError::MissingField {
                        ordinal: field.ordinal,
                        name: field.name.clone(),
                    });
                }
            }
        }
    }

    Ok(MaterializedStruct::new(layout.clone(), slots))
}

fn matches_tag(val: &DataValue, tag: CanonicalTypeTag) -> bool {
    match (val, tag) {
        (DataValue::Bool(_), CanonicalTypeTag::Bool) => true,
        (DataValue::Number(ExactNumber::Int(_)), CanonicalTypeTag::Int64) => true,
        (DataValue::Number(ExactNumber::UInt(_)), CanonicalTypeTag::UInt64) => true,
        (DataValue::Number(ExactNumber::Float(_)), CanonicalTypeTag::Float64) => true,
        (DataValue::Number(ExactNumber::Decimal128(_)), CanonicalTypeTag::Decimal128) => true,
        (DataValue::ObjectId(_), CanonicalTypeTag::ObjectId) => true,
        (DataValue::DateTimeUtc(_), CanonicalTypeTag::DateTimeUtc) => true,
        (DataValue::String(_), CanonicalTypeTag::StringSlice) => true,
        (DataValue::Binary(_), CanonicalTypeTag::BinarySlice) => true,
        (DataValue::Document(_), CanonicalTypeTag::NestedStruct(_)) => true,
        _ => false,
    }
}
