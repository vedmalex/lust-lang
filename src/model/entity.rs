use crate::model::lifecycle::{HookEvent, LifecycleHooks};
use crate::model::relation::{Relation, RelationKind};
use crate::schema::builder::Schema;

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
    pub fields: Vec<EntityField>,
    pub relations: Vec<Relation>,
    pub hooks: LifecycleHooks,
}

pub struct EntityBuilder {
    name: String,
    table_name: Option<String>,
    primary_key: String,
    fields: Vec<EntityField>,
    relations: Vec<Relation>,
    hooks: LifecycleHooks,
}

impl EntityBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        let name_str = name.into();
        let default_table = format!("{}s", name_str.to_lowercase());
        Self {
            name: name_str,
            table_name: Some(default_table),
            primary_key: "id".to_string(),
            fields: Vec::new(),
            relations: Vec::new(),
            hooks: LifecycleHooks::new(),
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

    pub fn build(self) -> Entity {
        Entity {
            name: self.name,
            table_name: self.table_name.unwrap_or_default(),
            primary_key: self.primary_key,
            fields: self.fields,
            relations: self.relations,
            hooks: self.hooks,
        }
    }
}
