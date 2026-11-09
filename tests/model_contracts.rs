use lust::model::lifecycle::HookEvent;
use lust::model::relation::RelationKind;
use lust::model::m;
use lust::schema::s;

#[test]
fn test_fluent_orm_entity_definition() {
    let user_entity = m
        .entity("User")
        .table("app_users")
        .primary_key("user_id")
        .field("user_id", s.int())
        .field("email", s.string().min(5))
        .field("is_active", s.bool())
        .has_many("posts", "Post", "author_id")
        .belongs_to("organization", "Organization", "org_id")
        .before_insert()
        .after_insert()
        .before_delete()
        .build();

    assert_eq!(user_entity.name, "User");
    assert_eq!(user_entity.table_name, "app_users");
    assert_eq!(user_entity.primary_key, "user_id");
    assert_eq!(user_entity.fields.len(), 3);
    assert_eq!(user_entity.relations.len(), 2);

    // Verify relations
    let has_many = &user_entity.relations[0];
    assert_eq!(has_many.name, "posts");
    assert_eq!(has_many.target_entity, "Post");
    assert_eq!(has_many.foreign_key, "author_id");
    assert_eq!(has_many.kind, RelationKind::HasMany);

    let belongs_to = &user_entity.relations[1];
    assert_eq!(belongs_to.name, "organization");
    assert_eq!(belongs_to.target_entity, "Organization");
    assert_eq!(belongs_to.foreign_key, "org_id");
    assert_eq!(belongs_to.kind, RelationKind::BelongsTo);

    // Verify registered lifecycle hooks
    assert!(user_entity.hooks.has_hook(&HookEvent::BeforeInsert));
    assert!(user_entity.hooks.has_hook(&HookEvent::AfterInsert));
    assert!(user_entity.hooks.has_hook(&HookEvent::BeforeDelete));
    assert!(!user_entity.hooks.has_hook(&HookEvent::BeforeUpdate));
}
