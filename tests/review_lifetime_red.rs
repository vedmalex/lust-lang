#![cfg(feature = "std")]

//! Independent executable falsifications for the 12A lifetime/budget review.

use lust::{BudgetTracker, DataView, LeaseError, OwnerId, OwnerRegistry};

#[test]
fn review_lifetime_red_foreign_runtime_owner_is_rejected() {
    let mut registry = OwnerRegistry::new(7);
    let valid = registry.allocate(42);
    let view = DataView::from_str("borrowed", valid);

    // Keep tenant/slot/generation valid but forge a different runtime namespace.
    let foreign_runtime = OwnerId::new(8, valid.tenant_id, valid.slot_id, valid.generation);
    let forged = DataView::from_str("foreign", foreign_runtime);

    assert!(view.as_str_checked(&registry, 42).is_ok());
    assert!(
        forged.as_str_checked(&registry, 42).is_err(),
        "a registry must reject an OwnerId issued by another runtime"
    );

}

#[test]
fn review_lifetime_red_forged_tenant_cannot_override_slot_tenant() {
    let mut registry = OwnerRegistry::new(7);
    let valid = registry.allocate(42);

    // Forge the tenant field to match the caller while retaining a slot
    // allocated to tenant 42. The registry must compare the handle to the slot.
    let foreign_tenant = OwnerId::new(7, 99, valid.slot_id, valid.generation);
    let forged = DataView::from_str("cross-tenant", foreign_tenant);
    assert_eq!(
        forged.as_str_checked(&registry, 99),
        Err(LeaseError::TenantMismatch {
            expected: 99,
            actual: 42,
        }),
        "caller and forged owner fields cannot override the tenant bound to the slot"
    );
}

#[test]
fn review_lifetime_red_zero_poll_interval_does_not_panic() {
    let outcome = std::panic::catch_unwind(|| {
        let mut tracker = BudgetTracker::new(8, 8, 1).with_poll_interval(0);
        tracker.charge_work(0)
    });

    assert!(
        outcome.is_ok(),
        "zero polling interval must be rejected or safely normalized, not panic"
    );
}
