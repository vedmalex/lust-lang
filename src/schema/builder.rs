use crate::data::DataValue;
use crate::schema::error::ValidationErrors;
use crate::schema::ir::{ObjectFieldIR, SchemaIR};
use crate::schema::validate::validate_ir;

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub ir: SchemaIR,
}

impl Schema {
    pub fn new(ir: SchemaIR) -> Self {
        Self { ir }
    }

    pub fn parse(&self, value: &DataValue) -> Result<DataValue, ValidationErrors> {
        let mut path = Vec::new();
        let mut errors = Vec::new();
        if let Some(val) = validate_ir(&self.ir, value, &mut path, &mut errors) {
            if errors.is_empty() {
                return Ok(val);
            }
        }
        Err(ValidationErrors::new(errors))
    }

    pub fn optional(self) -> Self {
        Schema::new(SchemaIR::Optional(Box::new(self.ir)))
    }

    pub fn nullable(self) -> Self {
        Schema::new(SchemaIR::Nullable(Box::new(self.ir)))
    }
}

pub struct StringBuilder {
    min_len: Option<usize>,
    max_len: Option<usize>,
    regex: Option<String>,
}

impl StringBuilder {
    pub fn new() -> Self {
        Self {
            min_len: None,
            max_len: None,
            regex: None,
        }
    }

    pub fn min(mut self, len: usize) -> Self {
        self.min_len = Some(len);
        self
    }

    pub fn max(mut self, len: usize) -> Self {
        self.max_len = Some(len);
        self
    }

    pub fn regex(mut self, pattern: impl Into<String>) -> Self {
        self.regex = Some(pattern.into());
        self
    }

    pub fn build(self) -> Schema {
        Schema::new(SchemaIR::String {
            min_len: self.min_len,
            max_len: self.max_len,
            regex: self.regex,
        })
    }
}

impl From<StringBuilder> for Schema {
    fn from(b: StringBuilder) -> Self {
        b.build()
    }
}

pub struct IntBuilder {
    min: Option<i64>,
    max: Option<i64>,
}

impl IntBuilder {
    pub fn new() -> Self {
        Self { min: None, max: None }
    }

    pub fn min(mut self, val: i64) -> Self {
        self.min = Some(val);
        self
    }

    pub fn max(mut self, val: i64) -> Self {
        self.max = Some(val);
        self
    }

    pub fn build(self) -> Schema {
        Schema::new(SchemaIR::Int {
            min: self.min,
            max: self.max,
        })
    }
}

impl From<IntBuilder> for Schema {
    fn from(b: IntBuilder) -> Self {
        b.build()
    }
}

pub struct FloatBuilder {
    min: Option<f64>,
    max: Option<f64>,
}

impl FloatBuilder {
    pub fn new() -> Self {
        Self { min: None, max: None }
    }

    pub fn min(mut self, val: f64) -> Self {
        self.min = Some(val);
        self
    }

    pub fn max(mut self, val: f64) -> Self {
        self.max = Some(val);
        self
    }

    pub fn build(self) -> Schema {
        Schema::new(SchemaIR::Float {
            min: self.min,
            max: self.max,
        })
    }
}

impl From<FloatBuilder> for Schema {
    fn from(b: FloatBuilder) -> Self {
        b.build()
    }
}

pub struct ArrayBuilder {
    item: SchemaIR,
    min_items: Option<usize>,
    max_items: Option<usize>,
}

impl ArrayBuilder {
    pub fn new(item: Schema) -> Self {
        Self {
            item: item.ir,
            min_items: None,
            max_items: None,
        }
    }

    pub fn min(mut self, count: usize) -> Self {
        self.min_items = Some(count);
        self
    }

    pub fn max(mut self, count: usize) -> Self {
        self.max_items = Some(count);
        self
    }

    pub fn build(self) -> Schema {
        Schema::new(SchemaIR::Array {
            item: Box::new(self.item),
            min_items: self.min_items,
            max_items: self.max_items,
        })
    }
}

impl From<ArrayBuilder> for Schema {
    fn from(b: ArrayBuilder) -> Self {
        b.build()
    }
}

pub struct ObjectBuilder {
    fields: Vec<ObjectFieldIR>,
    strict: bool,
}

impl ObjectBuilder {
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            strict: false,
        }
    }

    pub fn field(mut self, name: impl Into<String>, schema: impl Into<Schema>) -> Self {
        let sch = schema.into();
        let (schema_ir, optional) = match sch.ir {
            SchemaIR::Optional(inner) => (*inner, true),
            other => (other, false),
        };
        self.fields.push(ObjectFieldIR {
            name: name.into(),
            schema: schema_ir,
            optional,
            default: None,
        });
        self
    }

    pub fn field_with_default(
        mut self,
        name: impl Into<String>,
        schema: impl Into<Schema>,
        default_val: DataValue,
    ) -> Self {
        let sch = schema.into();
        self.fields.push(ObjectFieldIR {
            name: name.into(),
            schema: sch.ir,
            optional: true,
            default: Some(default_val),
        });
        self
    }

    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    pub fn build(self) -> Schema {
        Schema::new(SchemaIR::Object {
            fields: self.fields,
            strict: self.strict,
        })
    }
}

impl From<ObjectBuilder> for Schema {
    fn from(b: ObjectBuilder) -> Self {
        b.build()
    }
}

pub struct SchemaFactory;

#[allow(non_upper_case_globals)]
pub static s: SchemaFactory = SchemaFactory;

impl SchemaFactory {
    pub fn string(&self) -> StringBuilder {
        StringBuilder::new()
    }

    pub fn int(&self) -> IntBuilder {
        IntBuilder::new()
    }

    pub fn float(&self) -> FloatBuilder {
        FloatBuilder::new()
    }

    pub fn bool(&self) -> Schema {
        Schema::new(SchemaIR::Bool)
    }

    pub fn any(&self) -> Schema {
        Schema::new(SchemaIR::Any)
    }

    pub fn literal(&self, val: impl Into<DataValue>) -> Schema {
        Schema::new(SchemaIR::Literal(val.into()))
    }

    pub fn array(&self, item: impl Into<Schema>) -> ArrayBuilder {
        ArrayBuilder::new(item.into())
    }

    pub fn object(&self) -> ObjectBuilder {
        ObjectBuilder::new()
    }

    pub fn record(&self, key: impl Into<Schema>, value: impl Into<Schema>) -> Schema {
        Schema::new(SchemaIR::Record {
            key: Box::new(key.into().ir),
            value: Box::new(value.into().ir),
        })
    }

    pub fn union(&self, variants: Vec<Schema>) -> Schema {
        Schema::new(SchemaIR::Union(variants.into_iter().map(|sch| sch.ir).collect()))
    }

    pub fn discriminated_union(&self, discriminator: impl Into<String>, variants: Vec<Schema>) -> Schema {
        Schema::new(SchemaIR::DiscriminatedUnion {
            discriminator: discriminator.into(),
            variants: variants.into_iter().map(|sch| sch.ir).collect(),
        })
    }
}
