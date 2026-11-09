//! Fluent schema definition and validation engine for Lust.
//!
//! # Examples
//!
//! ```rust
//! use lust::schema::s;
//! use lust::data::{DataValue, Document, ExactNumber};
//!
//! let user_schema = s.object()
//!     .field("id", s.int().min(1).build())
//!     .field("name", s.string().min(1).max(50).build())
//!     .build();
//!
//! let mut doc = Document::new();
//! doc.try_insert("id", DataValue::Number(ExactNumber::Int(42))).unwrap();
//! doc.try_insert("name", DataValue::String("Alice".to_string())).unwrap();
//!
//! assert!(user_schema.parse(&DataValue::Document(doc)).is_ok());
//! ```

pub mod binding;
pub mod builder;
pub mod error;
pub mod ir;
pub mod validate;

pub use binding::{derive_schema_from_layout, StructSchemaBinding};
pub use builder::{s, ArrayBuilder, FloatBuilder, IntBuilder, ObjectBuilder, Schema, StringBuilder};
pub use error::{ValidationError, ValidationErrors};
pub use ir::{ObjectFieldIR, SchemaIR};
