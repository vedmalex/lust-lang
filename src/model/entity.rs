use crate::data::{DataValue, Document, ExactNumber, ObjectId};
use crate::model::lifecycle::{HookAction, HookError, HookEvent, LifecycleHooks};
use crate::model::relation::{Relation, RelationKind};
use crate::schema::builder::Schema;
use crate::schema::error::ValidationErrors;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub enum IdStrategy {
    ObjectId,
    AutoIncrement(Arc<AtomicI64>),
    Uuid,
    Custom(Arc<dyn Fn() -> DataValue + Send + Sync>),
}

impl std::fmt::Debug for IdStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdStrategy::ObjectId => write!(f, "IdStrategy::ObjectId"),
            IdStrategy::AutoIncrement(_) => write!(f, "IdStrategy::AutoIncrement"),
            IdStrategy::Uuid => write!(f, "IdStrategy::Uuid"),
            IdStrategy::Custom(_) => write!(f, "IdStrategy::Custom(<fn>)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    HookFailed(HookError),
    ValidationFailed(ValidationErrors),
    SerializationError(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::HookFailed(e) => write!(f, "Model hook failed: {}", e),
            ModelError::ValidationFailed(e) => write!(f, "Model validation failed: {:?}", e),
            ModelError::SerializationError(e) => write!(f, "Model serialization error: {}", e),
        }
    }
}

impl std::error::Error for ModelError {}

#[derive(Debug, Clone, PartialEq)]
pub enum SaveResult {
    Saved(ModelInstance),
    Cancelled,
}

impl SaveResult {
    pub fn is_saved(&self) -> bool {
        matches!(self, SaveResult::Saved(_))
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, SaveResult::Cancelled)
    }

    pub fn unwrap(self) -> ModelInstance {
        match self {
            SaveResult::Saved(instance) => instance,
            SaveResult::Cancelled => panic!("Called unwrap on a Cancelled SaveResult"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInstance {
    pub entity_name: String,
    pub document: Document,
}

impl ModelInstance {
    pub fn new(entity_name: impl Into<String>, document: Document) -> Self {
        Self {
            entity_name: entity_name.into(),
            document,
        }
    }

    pub fn primary_key<'a>(&'a self, entity: &Entity) -> Option<&'a DataValue> {
        self.document.get(&entity.primary_key)
    }

    pub fn to_db_params(&self, entity: &Entity) -> Vec<(String, DataValue)> {
        entity.to_db_params(&self.document)
    }

    pub fn to_bson(&self) -> Vec<u8> {
        crate::data::bson::to_vec(&self.document)
    }

    pub fn to_json(&self) -> Result<String, crate::data::json::JsonError> {
        crate::data::json::to_string(&DataValue::Document(self.document.clone()))
    }

    pub fn get(&self, field: &str) -> Option<&DataValue> {
        self.document.get(field)
    }

    pub fn set(&mut self, field: impl Into<String>, value: DataValue) -> Result<(), crate::data::DuplicateKeyError> {
        self.document.try_insert(field.into(), value)
    }
}

#[derive(Debug, Clone)]
pub struct EntityField {
    pub name: String,
    pub schema: Schema,
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub name: String,
    pub table_name: String,
    pub primary_key: String,
    pub id_strategy: Option<IdStrategy>,
    pub fields: Vec<EntityField>,
    pub relations: Vec<Relation>,
    pub hooks: LifecycleHooks,
    pub schema: Option<Schema>,
}

impl Entity {
    pub fn primary_key(&self) -> &str {
        &self.primary_key
    }

    pub fn get_primary_key<'a>(&self, doc: &'a Document) -> Option<&'a DataValue> {
        doc.get(&self.primary_key)
    }

    pub fn generate_id(&self) -> Option<DataValue> {
        static COUNTER: AtomicI64 = AtomicI64::new(1);
        match &self.id_strategy {
            Some(IdStrategy::ObjectId) => {
                let ts = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_secs() as u32,
                    Err(_) => 0,
                };
                let cnt = COUNTER.fetch_add(1, Ordering::SeqCst) as u64;
                let mut bytes = [0u8; 12];
                bytes[0..4].copy_from_slice(&ts.to_be_bytes());
                bytes[4..12].copy_from_slice(&cnt.to_be_bytes());
                Some(DataValue::ObjectId(ObjectId(bytes)))
            }
            Some(IdStrategy::AutoIncrement(counter)) => {
                let id = counter.fetch_add(1, Ordering::SeqCst);
                Some(DataValue::Number(ExactNumber::Int(id)))
            }
            Some(IdStrategy::Uuid) => {
                let cnt = COUNTER.fetch_add(1, Ordering::SeqCst);
                let ts = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_nanos(),
                    Err(_) => 0,
                };
                let uuid_str = format!("{:016x}{:016x}", ts, cnt);
                Some(DataValue::String(uuid_str))
            }
            Some(IdStrategy::Custom(generator)) => Some(generator()),
            None => None,
        }
    }

    /// Map fields to DB params without guessing inverse transform (explicitly maps struct/document fields to params).
    pub fn to_db_params(&self, doc: &Document) -> Vec<(String, DataValue)> {
        if !self.fields.is_empty() {
            let mut params = Vec::with_capacity(self.fields.len());
            for field in &self.fields {
                let val = doc.get(&field.name).cloned().unwrap_or(DataValue::Null);
                params.push((field.name.clone(), val));
            }
            params
        } else {
            doc.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
    }

    pub fn to_bson(&self, doc: &Document) -> Vec<u8> {
        crate::data::bson::to_vec(doc)
    }

    pub fn to_json(&self, doc: &Document) -> Result<String, crate::data::json::JsonError> {
        crate::data::json::to_string(&DataValue::Document(doc.clone()))
    }

    /// Execute save workflow:
    /// 1. Transactional staging copy (original is untouched)
    /// 2. Fixed-order before_save typed delegates
    ///    - cancellation skips the write and returns SaveResult::Cancelled
    ///    - failing hook aborts and discards staging (no partial struct published)
    /// 3. Assign generated ID if primary_key is absent or null
    /// 4. Validate against schema if present
    /// 5. Return SaveResult::Saved
    pub fn save(&self, doc: &Document) -> Result<SaveResult, ModelError> {
        let mut staging = doc.clone();

        match self.hooks.run_before_save(&mut staging) {
            Ok(HookAction::Cancel) => return Ok(SaveResult::Cancelled),
            Err(e) => return Err(ModelError::HookFailed(e)),
            Ok(HookAction::Continue) => {}
        }

        // Generate ID if missing or null
        if self.id_strategy.is_some() {
            let needs_id = match staging.get(&self.primary_key) {
                None => true,
                Some(DataValue::Null) => true,
                _ => false,
            };
            if needs_id {
                if let Some(id_val) = self.generate_id() {
                    let _ = staging.try_insert(self.primary_key.clone(), id_val);
                }
            }
        }

        if let Some(sch) = &self.schema {
            if let Err(errs) = sch.parse(&DataValue::Document(staging.clone())) {
                return Err(ModelError::ValidationFailed(errs));
            }
        }

        Ok(SaveResult::Saved(ModelInstance::new(self.name.clone(), staging)))
    }

    /// Execute load workflow:
    /// 1. Transactional staging copy
    /// 2. Fixed-order after_load typed delegates
    ///    - failing hook aborts and discards staging (no partial struct published)
    /// 3. Return ModelInstance
    pub fn load(&self, doc: &Document) -> Result<ModelInstance, ModelError> {
        let mut staging = doc.clone();

        if let Err(e) = self.hooks.run_after_load(&mut staging) {
            return Err(ModelError::HookFailed(e));
        }

        Ok(ModelInstance::new(self.name.clone(), staging))
    }
}

pub struct EntityBuilder {
    name: String,
    table_name: Option<String>,
    primary_key: String,
    id_strategy: Option<IdStrategy>,
    fields: Vec<EntityField>,
    relations: Vec<Relation>,
    hooks: LifecycleHooks,
    schema: Option<Schema>,
}

impl EntityBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        let name_str = name.into();
        let default_table = format!("{}s", name_str.to_lowercase());
        Self {
            name: name_str,
            table_name: Some(default_table),
            primary_key: "id".to_string(),
            id_strategy: None,
            fields: Vec::new(),
            relations: Vec::new(),
            hooks: LifecycleHooks::new(),
            schema: None,
        }
    }

    pub fn table(mut self, table: impl Into<String>) -> Self {
        self.table_name = Some(table.into());
        self
    }

    pub fn primary_key(mut self, pk: impl Into<String>) -> Self {
        self.primary_key = pk.into();
        self
    }

    pub fn generated_id(mut self, strategy: IdStrategy) -> Self {
        self.id_strategy = Some(strategy);
        self
    }

    pub fn auto_generate_id(mut self) -> Self {
        self.id_strategy = Some(IdStrategy::ObjectId);
        self
    }

    pub fn schema(mut self, schema: Schema) -> Self {
        self.schema = Some(schema);
        self
    }

    pub fn field(mut self, name: impl Into<String>, schema: impl Into<Schema>) -> Self {
        self.fields.push(EntityField {
            name: name.into(),
            schema: schema.into(),
        });
        self
    }

    pub fn has_many(
        mut self,
        name: impl Into<String>,
        target_entity: impl Into<String>,
        foreign_key: impl Into<String>,
    ) -> Self {
        self.relations.push(Relation::new(
            name,
            target_entity,
            foreign_key,
            RelationKind::HasMany,
        ));
        self
    }

    pub fn belongs_to(
        mut self,
        name: impl Into<String>,
        target_entity: impl Into<String>,
        foreign_key: impl Into<String>,
    ) -> Self {
        self.relations.push(Relation::new(
            name,
            target_entity,
            foreign_key,
            RelationKind::BelongsTo,
        ));
        self
    }

    pub fn before_insert(mut self) -> Self {
        self.hooks.register(HookEvent::BeforeInsert);
        self
    }

    pub fn after_insert(mut self) -> Self {
        self.hooks.register(HookEvent::AfterInsert);
        self
    }

    pub fn before_update(mut self) -> Self {
        self.hooks.register(HookEvent::BeforeUpdate);
        self
    }

    pub fn before_delete(mut self) -> Self {
        self.hooks.register(HookEvent::BeforeDelete);
        self
    }

    pub fn before_save<F>(mut self, hook: F) -> Self
    where
        F: Fn(&mut Document) -> Result<HookAction, HookError> + Send + Sync + 'static,
    {
        self.hooks.add_before_save(hook);
        self
    }

    pub fn on_before_save<F>(self, hook: F) -> Self
    where
        F: Fn(&mut Document) -> Result<HookAction, HookError> + Send + Sync + 'static,
    {
        self.before_save(hook)
    }

    pub fn after_load<F>(mut self, hook: F) -> Self
    where
        F: Fn(&mut Document) -> Result<(), HookError> + Send + Sync + 'static,
    {
        self.hooks.add_after_load(hook);
        self
    }

    pub fn on_after_load<F>(self, hook: F) -> Self
    where
        F: Fn(&mut Document) -> Result<(), HookError> + Send + Sync + 'static,
    {
        self.after_load(hook)
    }

    pub fn build(self) -> Entity {
        Entity {
            name: self.name,
            table_name: self.table_name.unwrap_or_default(),
            primary_key: self.primary_key,
            id_strategy: self.id_strategy,
            fields: self.fields,
            relations: self.relations,
            hooks: self.hooks,
            schema: self.schema,
        }
    }
}
