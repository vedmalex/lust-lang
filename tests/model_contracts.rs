use lust::data::{DataValue, Document, ExactNumber};
use lust::model::entity::{IdStrategy, ModelError, SaveResult};
use lust::model::lifecycle::{HookAction, HookError, HookEvent};
use lust::model::relation::RelationKind;
use lust::model::m;
use lust::schema::s;
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

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

#[test]
fn test_model_contract_primary_key_generated_ids_and_db_params() {
    let counter = Arc::new(AtomicI64::new(100));
    let entity = m
        .entity("Customer")
        .table("customers")
        .primary_key("cust_id")
        .generated_id(IdStrategy::AutoIncrement(counter.clone()))
        .field("cust_id", s.int())
        .field("name", s.string())
        .field("active", s.bool())
        .build();

    assert_eq!(entity.primary_key(), "cust_id");

    // Document without primary key
    let mut doc = Document::new();
    doc.try_insert("name".to_string(), DataValue::String("Acme".to_string())).unwrap();
    doc.try_insert("active".to_string(), DataValue::Bool(true)).unwrap();

    let save_res = entity.save(&doc).unwrap();
    let instance = save_res.unwrap();

    // Verify auto-generated id
    assert_eq!(
        instance.primary_key(&entity),
        Some(&DataValue::Number(ExactNumber::Int(100)))
    );

    // to_db_params: exact field mapping without guessing inverse transforms
    let db_params = instance.to_db_params(&entity);
    assert_eq!(db_params.len(), 3);
    assert_eq!(db_params[0], ("cust_id".to_string(), DataValue::Number(ExactNumber::Int(100))));
    assert_eq!(db_params[1], ("name".to_string(), DataValue::String("Acme".to_string())));
    assert_eq!(db_params[2], ("active".to_string(), DataValue::Bool(true)));

    // to_bson
    let bson_bytes = instance.to_bson();
    let decoded_doc = lust::data::bson::from_slice(&bson_bytes).unwrap();
    assert_eq!(decoded_doc.get("cust_id"), Some(&DataValue::Number(ExactNumber::Int(100))));

    // to_json
    let json_str = instance.to_json().unwrap();
    assert!(json_str.contains("\"cust_id\":100"));
    assert!(json_str.contains("\"name\":\"Acme\""));
}

#[test]
fn test_model_contract_object_id_generation() {
    let entity = m
        .entity("Device")
        .primary_key("_id")
        .auto_generate_id() // ObjectId
        .field("_id", s.any())
        .field("serial", s.string())
        .build();

    let mut doc = Document::new();
    doc.try_insert("serial".to_string(), DataValue::String("SN-9988".to_string())).unwrap();

    let instance = entity.save(&doc).unwrap().unwrap();
    let pk = instance.primary_key(&entity).unwrap();
    assert!(matches!(pk, DataValue::ObjectId(_)));
}

#[test]
fn test_hooks_before_save_fixed_order_cancellation_and_rollback() {
    let order_log = Arc::new(std::sync::Mutex::new(Vec::new()));

    let l1 = order_log.clone();
    let l2 = order_log.clone();

    let entity = m
        .entity("Article")
        .primary_key("id")
        .before_save(move |doc| {
            l1.lock().unwrap().push("hook_1");
            doc.try_insert("step1".to_string(), DataValue::Bool(true)).unwrap();
            Ok(HookAction::Continue)
        })
        .before_save(move |doc| {
            l2.lock().unwrap().push("hook_2");
            doc.try_insert("step2".to_string(), DataValue::Bool(true)).unwrap();
            Ok(HookAction::Continue)
        })
        .build();

    let doc = Document::new();
    let save_res = entity.save(&doc).unwrap();
    let instance = save_res.unwrap();

    // Verify order was fixed FIFO: hook_1 then hook_2
    assert_eq!(*order_log.lock().unwrap(), vec!["hook_1", "hook_2"]);
    assert_eq!(instance.get("step1"), Some(&DataValue::Bool(true)));
    assert_eq!(instance.get("step2"), Some(&DataValue::Bool(true)));

    // Test cancellation skips write
    let cancel_entity = m
        .entity("Article")
        .before_save(|_| Ok(HookAction::Cancel))
        .build();

    let res = cancel_entity.save(&Document::new()).unwrap();
    assert_eq!(res, SaveResult::Cancelled);

    // Test failing hook does NOT leave a partial struct published
    let mut original_doc = Document::new();
    original_doc.try_insert("status".to_string(), DataValue::String("initial".to_string())).unwrap();

    let failing_entity = m
        .entity("Article")
        .before_save(|doc| {
            // Partially mutate doc
            let _ = doc.try_insert("corrupted".to_string(), DataValue::Bool(true));
            Err(HookError::new("validation failed in hook"))
        })
        .build();

    let fail_res = failing_entity.save(&original_doc);
    assert!(matches!(fail_res, Err(ModelError::HookFailed(_))));

    // Verify original doc was NOT mutated or partially published
    assert_eq!(original_doc.get("status"), Some(&DataValue::String("initial".to_string())));
    assert_eq!(original_doc.get("corrupted"), None);
}

#[test]
fn test_hooks_after_load_fixed_order_and_failure_aborts() {
    let order_log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let l1 = order_log.clone();
    let l2 = order_log.clone();

    let entity = m
        .entity("Profile")
        .after_load(move |doc| {
            l1.lock().unwrap().push("after_1");
            doc.try_insert("computed_score".to_string(), DataValue::Number(ExactNumber::Int(100))).unwrap();
            Ok(())
        })
        .after_load(move |doc| {
            l2.lock().unwrap().push("after_2");
            doc.try_insert("loaded_at".to_string(), DataValue::String("2026-11-09".to_string())).unwrap();
            Ok(())
        })
        .build();

    let raw_doc = Document::new();
    let instance = entity.load(&raw_doc).unwrap();

    assert_eq!(*order_log.lock().unwrap(), vec!["after_1", "after_2"]);
    assert_eq!(instance.get("computed_score"), Some(&DataValue::Number(ExactNumber::Int(100))));
    assert_eq!(instance.get("loaded_at"), Some(&DataValue::String("2026-11-09".to_string())));

    // Test failing after_load hook aborts and does not leave partial struct published
    let failing_entity = m
        .entity("Profile")
        .after_load(|_| Err(HookError::new("decryption failed")))
        .build();

    let load_res = failing_entity.load(&raw_doc);
    assert!(matches!(load_res, Err(ModelError::HookFailed(_))));
}
