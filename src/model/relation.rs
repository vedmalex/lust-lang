#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationKind {
    HasMany,
    BelongsTo,
    HasOne,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    pub target_entity: String,
    pub foreign_key: String,
    pub kind: RelationKind,
}

impl Relation {
    pub fn new(
        name: impl Into<String>,
        target_entity: impl Into<String>,
        foreign_key: impl Into<String>,
        kind: RelationKind,
    ) -> Self {
        Self {
            name: name.into(),
            target_entity: target_entity.into(),
            foreign_key: foreign_key.into(),
            kind,
        }
    }
}
