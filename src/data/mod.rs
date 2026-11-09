//! Data model, formats, zero-copy views, and budget tracking for Lust.
//!
//! # Examples
//!
//! ```rust
//! use lust::data::{Document, DataValue, ExactNumber};
//!
//! let mut doc = Document::new();
//! doc.try_insert("id", DataValue::Number(ExactNumber::Int(1))).unwrap();
//! doc.try_insert("name", DataValue::String("Lust".to_string())).unwrap();
//!
//! // Gate-SCHEMA-01: Duplicate keys are strictly rejected
//! assert!(doc.try_insert("id", DataValue::Number(ExactNumber::Int(2))).is_err());
//! ```

pub mod bson;
pub mod budget;
pub mod document;
pub mod json;
pub mod jsonc;
pub mod jsonl;
pub mod layout;
pub mod materialize;
pub mod number;
pub mod owner;
pub mod toon;
pub mod value;
pub mod view;
pub mod yaml;

pub use bson::{BsonArrayView, BsonDocumentView, BsonElementIter, BsonError, BsonValueView};
pub use budget::{BudgetError, BudgetTracker};
pub use document::{Document, DuplicateKeyError};
pub use jsonl::{
    line_results, lines, lines_with_options, validate_line_results, validate_lines, write_lines,
    BlankLines, CancellationToken, ControlFlow, JsonlReader, JsonlReaderOptions, JsonlResults,
    JsonlValidated, JsonlValidatedResults, JsonlWriter, JsonlWriterOptions, LineError,
    LineErrorKind, StreamValidationError, StreamWriteError, WriteErrorKind,
};
pub use layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1, TargetAbi};
pub use materialize::{
    materialize_from_document, materialize_from_item, materialize_into_struct, struct_def_to_layout,
    MaterializeError, MaterializedStruct,
};
pub use number::{DataError, DateTimeUtc, Decimal128, ExactNumber, ObjectId};
pub use owner::{LeaseError, LeaseLifecycleState, OwnerId, OwnerRegistry, SlotEntry};
pub use toon::{ToonCodec, ToonError, ToonTable};
pub use value::DataValue;
pub use view::DataView;
pub use yaml::{LustSafeYaml12, YamlError};


