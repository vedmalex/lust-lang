# Gate-SCHEMA-12 Stage 2 — Luna Independent DA/LV Review

**Target:** `repos/lust-lang-fork`, branch `schema/gate-12`, HEAD `5a99ade69cfeb69cfa657ce1d0c1b52b9634a177`  
**Normative reference:** `docs/schema/reviews/GATE-12-ASTRA-PLAN-20261110.md`  
**Verdict:** **CONCERNS**  
**Evidence status:** Source inspection and requested test targets confirmed; adversarial race schedules and mutant rejection remain unverified.

## Execution

| Command | Result |
|---|---|
| `cargo test --test rx_lifecycle` | PASS — 5 passed, 0 failed |
| `cargo test --test rx_scheduler` | PASS — 11 passed, 0 failed |

The lifecycle tests cover normal and failed termination, cancellation before scheduler execution, self-cancel/self-join in a synchronous callback, and interval cancellation. The scheduler tests cover capacity, virtual time, timers, and coordinator admission/quiescence.

## Findings

1. **Cancellation does not serialize callback admission with callback start — IN-PATH, CONFIRMED.** In `SubscriptionControl::deliver_next`, Active is checked under the state mutex, then the mutex is released before `current_executing_thread` is set and before `on_next` is entered. `cancel` can transition to Stopping in that gap and return, after which the already-approved `deliver_next` starts a new callback. This violates the plan’s linearized “no new callback starts after cancel commits” contract. The current cancellation test cancels before producer execution and does not exercise this interleaving. Relevant source: `src/rx/lifecycle.rs`, `cancel` and `deliver_next`.

2. **Self-cancel from within on_next deadlocks — IN-PATH, CONFIRMED by lock structure.** `deliver_next` holds `observer` mutex while invoking user `on_next`. Calling `cancel` there acquires `state`, then `exit_status`, then coordinator locks, so it itself can return. But terminal caveat: cancellation works; subsequent self-join is checked before state lock and returns SelfJoin. However, if user callback calls complete/fail recursively, those try to lock observer and deadlock. For requested self-unsubscribe specifically, no deadlock is apparent in implementation, and the test passes. **Status: PASS for the tested self-cancel/self-join path**, with untested reentrant terminal calls as a distinct concern.

3. **Terminal outcome is not fully idempotent — IN-PATH, CONFIRMED.** `complete`/`fail` only write the outcome slot if empty, but always execute `terminal_dispatched.swap(true)`. A duplicate call after cancellation or after a prior terminal outcome may consume the dispatch bit. Worse, terminal callback is invoked while holding state and observer locks. Sequential duplicate calls after a committed terminal callback leave user callback count at one, but concurrent complete/fail/cancel races are not serialized as a single outcome+dispatch transition. The tests validate sequential once-only behavior, not race permutations promised by the plan.

4. **Interval cancellation quiescence — PARTIAL.** Interval registers a ticker participant; an already-dispatched tick sees inactive and acknowledges exit; a delivered tick that fails also acknowledges. Join closes admission and waits on participant count. The test cancels, advances/drains timers, then joins. If cancellation occurs after timer expiry is polled/enqueued but before its callback, eventual acknowledgement requires scheduler execution; the current test explicitly drains first. This supports quiescence conditional on scheduler progress, not unconditional ticker cleanup.

5. **Scope admission closure race — IN-PATH, CONFIRMED.** `register_participant` locks `closed_scopes`, checks it, then separately locks `participants`; `close_admission` only locks the first mutex. Thus close can occur between check and insertion, admitting a participant after closure. This contradicts the documented closed-admission guarantee. Current test checks closed admission without a forced concurrent interleaving.

6. **Subscribe scheduling failure leaks registered participant — IN-PATH, CONFIRMED.** `Observable::subscribe` registers a participant before `schedule_task`; on scheduling error it returns BackpressureOverflow without acknowledging/removing that participant. A subsequent join can wait/time out forever. This is a registration rollback defect.

7. **Terminal callback lock/reentrancy risk — IN-PATH, CONFIRMED.** `complete` and `fail` invoke user terminal callbacks while holding `state` and `observer` mutexes. A terminal callback that re-enters lifecycle operations can deadlock; this also contradicts the normative instruction that no lock is held while invoking user code.

8. **Scope closure sharing — DESIGN/CONTRACT CONCERN.** Cancellation closes admission for the entire numeric scope ID. Reusing that scope ID for later subscriptions is permanently rejected, while closed-scope entries are never retired. Verify intended scope lifetime and bounded bookkeeping before treating numeric scope IDs as reusable.

## Falsification checklist

- Cancellation zero-callback contract: **PARTIAL / CONCERN** — zero terminal callbacks for the simple pre-start case passes; callback-start race falsifies the stronger contract by source interleaving.
- Self-unsubscribe / no deadlock: **PASS for tested path** — lifecycle test passes, including SelfJoin from on_next.
- Duplicate terminal idempotence: **PARTIAL / CONCERN** — sequential test coverage passes; competing outcome races and reentrant callback cases are not tested.
- Interval cancellation join waits for ticker acknowledgement: **PARTIAL** — participant accounting exists and drained virtual path is tested; races requiring pending scheduler progress are not exercised.
- Closed participant admission: **CONCERN** — check/insert not atomic with closure.
- Bounded ready task queue: **PARTIAL** — capacity tested; `step` silently discards expired timer actions when ready queue admission fails.

## Overall assessment

The requested tests are green, but they do not establish the full Stage 2 lifecycle contract. Findings 1, 5, 6, and 7 are implementation defects directly relevant to cancellation/quiescence and safe lifecycle behavior. No Stage 2 gate closure is supported by this review alone. No mutants were built or run; this review used source inspection and the requested test executions only.
