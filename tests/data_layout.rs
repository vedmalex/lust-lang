#![cfg(feature = "std")]

use lust::{
    BudgetError, BudgetTracker, CanonicalTypeTag, LogicalFieldDesc, LogicalLayoutV1,
};

#[test]
fn test_logical_layout_c_flat_calculation() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "active", CanonicalTypeTag::Bool),
        LogicalFieldDesc::new(2, "score", CanonicalTypeTag::Float64),
    ];

    let layout = LogicalLayoutV1::new(1001, "UserMetrics", fields);
    assert_eq!(layout.field_count(), 3);
    assert!(layout.is_c_flat);
    assert!(layout.assert_c_flat().is_ok());

    // Field 0: offset 0, size 8
    let f0 = layout.field_by_ordinal(0).unwrap();
    assert_eq!(f0.byte_offset, 0);
    assert_eq!(f0.byte_size, 8);

    // Field 1: offset 8, size 1 (bool)
    let f1 = layout.field_by_ordinal(1).unwrap();
    assert_eq!(f1.byte_offset, 8);
    assert_eq!(f1.byte_size, 1);

    // Field 2: alignment 8 => padded to offset 16, size 8
    let f2 = layout.field_by_ordinal(2).unwrap();
    assert_eq!(f2.byte_offset, 16);
    assert_eq!(f2.byte_size, 8);

    // Total size aligned to max_align 8: 16 + 8 = 24
    assert_eq!(layout.size_bytes, 24);
    assert_eq!(layout.align_bytes, 8);
}

#[test]
fn test_logical_layout_managed_slice_non_c_flat() {
    let fields = vec![
        LogicalFieldDesc::new(0, "id", CanonicalTypeTag::Int64),
        LogicalFieldDesc::new(1, "name", CanonicalTypeTag::StringSlice),
    ];

    let layout = LogicalLayoutV1::new(1002, "UserProfile", fields);
    assert_eq!(layout.field_count(), 2);
    // Contains managed slice => cannot be treated as raw C flat struct
    assert!(!layout.is_c_flat);
    assert!(layout.assert_c_flat().is_err());

    let f1 = layout.field_by_name("name").unwrap();
    assert_eq!(f1.ordinal, 1);
    assert_eq!(f1.byte_size, 24); // (ptr, len, OwnerId)
}

#[test]
fn test_budget_tracker_memory_quota_boundary_triplet() {
    let max_bytes = 1000;
    let mut tracker = BudgetTracker::new(max_bytes, 100_000, 10);

    // 1. Quota - 1: Success
    assert!(tracker.charge_bytes(999).is_ok());
    assert_eq!(tracker.allocated_bytes(), 999);

    // 2. Exact Quota: Success (allocating remaining 1 byte)
    assert!(tracker.charge_bytes(1).is_ok());
    assert_eq!(tracker.allocated_bytes(), 1000);

    // 3. Quota + 1: Fails closed (Gate-SCHEMA-02)
    let err = tracker.charge_bytes(1);
    assert_eq!(
        err,
        Err(BudgetError::BytesExceeded {
            limit: 1000,
            requested: 1001,
        })
    );

    // Release bytes allows new allocations
    tracker.release_bytes(500);
    assert_eq!(tracker.allocated_bytes(), 500);
    assert!(tracker.charge_bytes(200).is_ok());
    assert_eq!(tracker.allocated_bytes(), 700);
}

#[test]
fn test_budget_tracker_work_units_and_polling_interval() {
    let mut tracker = BudgetTracker::new(100_000, 10_000, 10).with_poll_interval(4096);

    // Charging less than interval: returns false (no poll needed)
    let need_poll_1 = tracker.charge_work(4000).expect("charge work");
    assert!(!need_poll_1);

    // Crossing 4096 threshold: returns true (cooperative cancellation poll triggered)
    let need_poll_2 = tracker.charge_work(100).expect("charge work");
    assert!(need_poll_2);

    // Exceeding total work units: fails closed
    let err = tracker.charge_work(6000);
    assert_eq!(
        err,
        Err(BudgetError::WorkUnitsExceeded {
            limit: 10_000,
            current: 10_100,
        })
    );
}

#[test]
fn test_budget_tracker_depth_limit() {
    let mut tracker = BudgetTracker::new(100_000, 100_000, 3);

    assert!(tracker.enter_depth().is_ok()); // depth 1
    assert!(tracker.enter_depth().is_ok()); // depth 2
    assert!(tracker.enter_depth().is_ok()); // depth 3
    assert_eq!(tracker.current_depth(), 3);

    // Depth 4 exceeds max_depth 3: fails closed
    let err = tracker.enter_depth();
    assert_eq!(
        err,
        Err(BudgetError::DepthExceeded {
            limit: 3,
            current: 4,
        })
    );

    tracker.exit_depth();
    assert_eq!(tracker.current_depth(), 2);
    assert!(tracker.enter_depth().is_ok());
}
