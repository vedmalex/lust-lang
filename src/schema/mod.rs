pub mod binding;
pub mod builder;
pub mod error;
pub mod ir;
pub mod validate;

pub use binding::{derive_schema_from_layout, StructSchemaBinding};
pub use builder::{s, ArrayBuilder, FloatBuilder, IntBuilder, ObjectBuilder, Schema, StringBuilder};
pub use error::{ValidationError, ValidationErrors};
pub use ir::{ObjectFieldIR, SchemaIR};
