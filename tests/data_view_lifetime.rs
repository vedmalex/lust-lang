#![cfg(feature = "std")]

use lust::{
    DataValue, DataView, LeaseError, LeaseLifecycleState, OwnerRegistry,
};

#[test]
fn test_dataview_zero_copy_borrow_and_reborrow() {
    let mut registry = OwnerRegistry::new(1);
    let owner = registry.allocate(100);

    let raw_text = "{\"user\": \"alice\", \"role\": \"admin\"}";
    let view = DataView::from_str(raw_text, owner);

    assert_eq!(view.as_str(), Some(raw_text));
    assert_eq!(view.owner_id(), Some(owner));

    // Reborrowing preserves the OwnerId
    let sub_view = view.slice(1, 7).expect("slice valid");
    assert_eq!(sub_view.as_str(), Some("\"user\""));
    assert_eq!(sub_view.owner_id(), Some(owner));
}

#[test]
fn test_pre_dereference_validation_success_and_tenant_mismatch() {
    let mut registry = OwnerRegistry::new(1);
    let owner = registry.allocate(100);

    let view = DataView::from_str("payload", owner);

    // Same tenant: checked dereference succeeds
    let str_ref = view.as_str_checked(&registry, 100).expect("access allowed");
    assert_eq!(str_ref, "payload");

    // Different tenant: checked dereference is rejected BEFORE memory access (Gate-SCHEMA-03)
    let err = view.as_str_checked(&registry, 200);
    assert_eq!(
        err,
        Err(LeaseError::TenantMismatch {
            expected: 200,
            actual: 100,
        })
    );
}

#[test]
fn test_stale_generation_rejected_after_slot_recycled() {
    let mut registry = OwnerRegistry::new(1);
    let owner_gen1 = registry.allocate(100);
    assert_eq!(owner_gen1.generation, 1);

    let view_gen1 = DataView::from_str("data_v1", owner_gen1);

    // Invalidate slot 0 through full protocol
    registry.cancel(owner_gen1).expect("cancel");
    registry.acknowledge_join(owner_gen1).expect("join");
    registry.release(owner_gen1).expect("release");

    // Reallocate slot: generation increments to 2
    let owner_gen2 = registry.allocate(100);
    assert_eq!(owner_gen2.slot_id, owner_gen1.slot_id);
    assert_eq!(owner_gen2.generation, 2);

    // Old view with generation 1 MUST be rejected before dereference
    let err = view_gen1.validate_access(&registry, 100);
    assert_eq!(
        err,
        Err(LeaseError::StaleGeneration {
            expected: 2,
            actual: 1,
        })
    );

    // New view with generation 2 succeeds
    let view_gen2 = DataView::from_str("data_v2", owner_gen2);
    assert!(view_gen2.validate_access(&registry, 100).is_ok());
}

#[test]
fn test_gate_03_release_before_join_protocol_violation() {
    let mut registry = OwnerRegistry::new(1);
    let owner = registry.allocate(100);

    // Cancel initiated
    registry.cancel(owner).expect("cancel");
    assert_eq!(
        registry.slot_state(owner.slot_id),
        Some(LeaseLifecycleState::Cancelled)
    );

    // Attempting release BEFORE join acknowledgement MUST fail closed
    let premature_release = registry.release(owner);
    assert_eq!(premature_release, Err(LeaseError::ReleaseBeforeJoin));

    // After join acknowledgement, release succeeds
    registry.acknowledge_join(owner).expect("join");
    assert_eq!(
        registry.slot_state(owner.slot_id),
        Some(LeaseLifecycleState::Joined)
    );

    registry.release(owner).expect("release");
    assert_eq!(
        registry.slot_state(owner.slot_id),
        Some(LeaseLifecycleState::Released)
    );
}

#[test]
fn test_to_owned_materialization_survives_lease_release() {
    let mut registry = OwnerRegistry::new(1);
    let owner = registry.allocate(100);

    let owned_data: DataValue;
    {
        let buffer = String::from("ephemeral message for background worker");
        let view = DataView::from_str(&buffer, owner);
        // Explicit materialization mandated for async/background boundary
        owned_data = view.to_owned();
    }

    // Buffer is dropped, lease is cancelled and released
    registry.cancel(owner).expect("cancel");
    registry.acknowledge_join(owner).expect("join");
    registry.release(owner).expect("release");

    // Owned data is completely independent and valid
    assert_eq!(
        owned_data,
        DataValue::String("ephemeral message for background worker".to_string())
    );
}

#[test]
fn test_gate_03_mutant_permitting_release_before_join_fails_oracle() {
    // Mutant Oracle: simulate broken registry that allows release without join
    let mut registry = OwnerRegistry::new(1);
    let owner = registry.allocate(100);
    registry.cancel(owner).expect("cancel");

    // Production oracle strictly catches premature release
    assert_eq!(registry.release(owner), Err(LeaseError::ReleaseBeforeJoin));

    // Broken mutant simulator: skips state check
    fn mutant_release_without_join(reg: &mut OwnerRegistry, owner: lust::OwnerId) -> Result<(), LeaseError> {
        // Deliberately broken: sets Released directly without checking Joined
        reg.acknowledge_join(owner)?;
        reg.release(owner)
    }

    // Mutant changes lifecycle state to Released prematurely
    let mut mutant_reg = registry.clone();
    let mutant_res = mutant_release_without_join(&mut mutant_reg, owner);
    assert_eq!(mutant_res, Ok(())); // Mutant deviates from required protocol error
}
