//! Example 5: Dependency Injection & Mocking via StackContextProjection.
//! Demonstrates zero-heap stack-allocated context injection, dynamic MockService spies,
//! call verification, and safe service extraction in Effect-TS / ZIO style.

use lust::effect_abi::{Cause, Exit, StackContextProjection};
use lust::test_framework::{
    assert_eq, assert_fails, assert_success, MockService,
};

const TAG_DATABASE: u32 = 1001;
const TAG_HTTP_CLIENT: u32 = 1002;

/// Data model for an incoming checkout order.
struct CheckoutOrder {
    order_id: &'static str,
    user_id: &'static str,
    amount_cents: u64,
}

/// Business Effect: Processes order payment via HTTP API and persists receipt in Database.
/// Consumes dependencies purely through `StackContextProjection<N>` on the stack.
fn process_checkout<const N: usize>(
    ctx: &StackContextProjection<N>,
    order: &CheckoutOrder,
) -> Exit<String, &'static str> {
    // 1. Resolve Database dependency from zero-heap projection
    let db_ptr = match ctx.get(TAG_DATABASE) {
        Some(ptr) => ptr,
        None => return Exit::Failure(Cause::Fail("Context missing Database service")),
    };
    let db = unsafe { &*(db_ptr as *const MockService) };

    // 2. Resolve HTTP Client dependency from zero-heap projection
    let http_ptr = match ctx.get(TAG_HTTP_CLIENT) {
        Some(ptr) => ptr,
        None => return Exit::Failure(Cause::Fail("Context missing HTTP Client service")),
    };
    let http = unsafe { &*(http_ptr as *const MockService) };

    // 3. Step 1: Query user status and credit in DB
    println!("  [Step 1] Querying database for user: {}", order.user_id);
    db.record_call("get_user", &[order.user_id], 10);

    // 4. Step 2: Authorize charge via external HTTP payment gateway
    let amount_str = order.amount_cents.to_string();
    println!(
        "  [Step 2] Sending HTTP POST to payment gateway (amount: {} cents)",
        order.amount_cents
    );
    http.record_call(
        "POST /v1/payments/charge",
        &[order.order_id, &amount_str],
        25,
    );

    // 5. Step 3: Record transaction in database
    println!(
        "  [Step 3] Persisting transaction record for order: {}",
        order.order_id
    );
    db.record_call(
        "insert_order",
        &[order.order_id, order.user_id, "PAID"],
        40,
    );

    Exit::Success(format!("ORDER_COMPLETED:{}", order.order_id))
}

fn main() {
    println!("--- [Example 5: Dependency Injection & Mocking via StackContextProjection] ---");

    // -----------------------------------------------------------------------
    // Part 1: Initializing Dynamic Mock Services
    // -----------------------------------------------------------------------
    println!("\n1. Creating MockService instances for Database (tag 1001) and HTTP (tag 1002):");
    let mock_db = MockService::new(TAG_DATABASE);
    let mock_http = MockService::new(TAG_HTTP_CLIENT);

    // Configure expected responses on mocks
    mock_http.set_response(
        "POST /v1/payments/charge",
        "{\"status\": \"APPROVED\", \"charge_id\": \"ch_999\"}".to_string(),
    );

    // -----------------------------------------------------------------------
    // Part 2: Assembling Zero-Heap StackContextProjection
    // -----------------------------------------------------------------------
    println!("\n2. Projecting dependencies onto the stack (Zero-Heap Allocation):");
    let projection = MockService::to_projection_pair(&mock_db, &mock_http);

    println!(
        "  StackContextProjection<2> size in bytes on stack: {}",
        std::mem::size_of_val(&projection)
    );
    println!("  Verified pointer lookup for TAG_DATABASE (1001): {:?}", projection.get(TAG_DATABASE).is_some());
    println!("  Verified pointer lookup for TAG_HTTP_CLIENT (1002): {:?}", projection.get(TAG_HTTP_CLIENT).is_some());
    println!("  Verified unknown service (9999) returns None: {:?}", projection.get(9999));

    // -----------------------------------------------------------------------
    // Part 3: Executing Business Effect with Injected Context
    // -----------------------------------------------------------------------
    println!("\n3. Executing order checkout effect with mocked dependencies:");
    let order = CheckoutOrder {
        order_id: "ord_2026_x89",
        user_id: "usr_alice",
        amount_cents: 4500,
    };

    let result = process_checkout(&projection, &order);
    let outcome = assert_success(&result, None).unwrap();
    println!("  Checkout succeeded with outcome: {}", outcome);
    assert_eq(outcome.as_str(), "ORDER_COMPLETED:ord_2026_x89", None).unwrap();

    // -----------------------------------------------------------------------
    // Part 4: Verifying Mock Call Spies and Arguments
    // -----------------------------------------------------------------------
    println!("\n4. Verifying mock call invocations, counts, and parameters:");
    assert_eq(mock_db.call_count("get_user"), 1, None).unwrap();
    assert_eq(mock_db.call_count("insert_order"), 1, None).unwrap();
    assert_eq(mock_http.call_count("POST /v1/payments/charge"), 1, None).unwrap();
    println!("  [PASS] All expected method calls registered!");

    let last_db_call = mock_db.last_call("insert_order").expect("insert_order should have been recorded");
    println!("  Database last_call arguments: {:?}", last_db_call.args);
    println!("  Database last_call timestamp: {} ms", last_db_call.timestamp_ms);
    assert_eq(
        last_db_call.args,
        vec!["ord_2026_x89".to_string(), "usr_alice".to_string(), "PAID".to_string()],
        None,
    ).unwrap();

    let last_http_call = mock_http.last_call("POST /v1/payments/charge").expect("HTTP call recorded");
    println!("  HTTP client last_call arguments: {:?}", last_http_call.args);
    assert_eq(
        last_http_call.args,
        vec!["ord_2026_x89".to_string(), "4500".to_string()],
        None,
    ).unwrap();

    // -----------------------------------------------------------------------
    // Part 5: Negative Testing (Missing Context Requirement)
    // -----------------------------------------------------------------------
    println!("\n5. Testing fail-closed semantics on missing dependency:");
    // Single-service projection with only Database (missing HTTP client):
    let partial_projection = mock_db.to_projection_single();
    let partial_result = process_checkout(&partial_projection, &order);

    let err_msg = assert_fails(&partial_result, Some(&"Context missing HTTP Client service"), None).unwrap();
    println!("  Captured expected missing context error: \"{}\"", err_msg);

    println!("\n[PASS] Dependency Injection and Mocking verified successfully!");
}
