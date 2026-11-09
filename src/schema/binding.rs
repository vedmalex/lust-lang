use crate::data::layout::{CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1};
use crate::schema::builder::Schema;
use crate::schema::ir::{ObjectFieldIR, SchemaIR};

#[derive(Debug, Clone, PartialEq)]
pub struct StructSchemaBinding {
    pub layout: LogicalLayoutV1,
    pub schema: Schema,
}

impl StructSchemaBinding {
    pub fn new(layout: LogicalLayoutV1) -> Self {
        let schema = derive_schema_from_layout(&layout);
        Self { layout, schema }
    }

    pub fn get_field_by_ordinal(&self, ordinal: u16) -> Option<&LogicalFieldDesc> {
        self.layout.fields.iter().find(|f| f.ordinal == ordinal)
    }

    pub fn get_field_by_name(&self, name: &str) -> Option<&LogicalFieldDesc> {
        self.layout.fields.iter().find(|f| f.name == name)
    }
}

pub fn derive_schema_from_layout(layout: &LogicalLayoutV1) -> Schema {
    let mut fields = Vec::with_capacity(layout.fields.len());

    for field in &layout.fields {
        let mut field_schema = match field.tag {
            CanonicalTypeTag::Bool => SchemaIR::Bool,
            CanonicalTypeTag::Int64 | CanonicalTypeTag::UInt64 => SchemaIR::Int { min: None, max: None },
            CanonicalTypeTag::Float64 => SchemaIR::Float { min: None, max: None },
            CanonicalTypeTag::StringSlice => SchemaIR::String { min_len: None, max_len: None, regex: None },
            CanonicalTypeTag::BinarySlice | CanonicalTypeTag::ObjectId | CanonicalTypeTag::DateTimeUtc | CanonicalTypeTag::Decimal128 => SchemaIR::Any,
            CanonicalTypeTag::NestedStruct(_) => SchemaIR::Any,
        };

        if field.is_nullable {
            field_schema = SchemaIR::Nullable(Box::new(field_schema));
        }

        fields.push(ObjectFieldIR {
            name: field.name.clone(),
            schema: field_schema,
            optional: field.is_optional,
            default: None,
        });
    }

    Schema::new(SchemaIR::Object {
        fields,
        strict: true,
    })
}
