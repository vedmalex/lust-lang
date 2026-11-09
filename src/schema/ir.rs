use crate::data::DataValue;

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectFieldIR {
    pub name: String,
    pub schema: SchemaIR,
    pub optional: bool,
    pub default: Option<DataValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SchemaIR {
    Any,
    String {
        min_len: Option<usize>,
        max_len: Option<usize>,
        regex: Option<String>,
    },
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    Float {
        min: Option<f64>,
        max: Option<f64>,
    },
    Bool,
    Binary {
        min_len: Option<usize>,
        max_len: Option<usize>,
    },
    ObjectId,
    DateTimeUtc,
    Decimal128,
    Literal(DataValue),
    Array {
        item: Box<SchemaIR>,
        min_items: Option<usize>,
        max_items: Option<usize>,
    },
    Object {
        fields: Vec<ObjectFieldIR>,
        strict: bool,
    },
    Record {
        key: Box<SchemaIR>,
        value: Box<SchemaIR>,
    },
    Union(Vec<SchemaIR>),
    DiscriminatedUnion {
        discriminator: String,
        variants: Vec<SchemaIR>,
    },
    Optional(Box<SchemaIR>),
    Nullable(Box<SchemaIR>),
}
