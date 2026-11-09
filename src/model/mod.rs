pub mod entity;
pub mod lifecycle;
pub mod relation;

pub use entity::{Entity, EntityBuilder, EntityField};
pub use lifecycle::{HookEvent, LifecycleHooks};
pub use relation::{Relation, RelationKind};

pub struct ModelFactory;

#[allow(non_upper_case_globals)]
pub static m: ModelFactory = ModelFactory;

impl ModelFactory {
    pub fn entity(&self, name: impl Into<String>) -> EntityBuilder {
        EntityBuilder::new(name)
    }
}
