# Gate-SCHEMA-12 — Master Implementation and Verification Brief

Date: 2026-11-10. Task: **GATE-12-ASTRA-PLAN**. Author: Astra, design sub-agent.

**Status: CONCERNS — implementation-ready proposal, pending coordinator design acceptance and runtime evidence. Gate 12 remains open.** This report performs static inspection, not execution or certification. No runtime tests or mutants were run for this brief. The existing unconditional panic was inspected and is not changed here. Other gates' acceptance is contextual input, not independently recertified.

## 1. Scope, provenance and architectural decision

- Evaluation checkout: `main`, `5a9853f9b9e7456e1d58ff0352cd443fe8e61b6b`. Pre-existing untracked reports and `indexes/` preserved.
- Compiler checkout: `repos/lust-lang-fork`, clean at inspection, `89c83bae6744c385ad3bbc15d2c82532efd1ca2b`; verified branch `perf/10x-jit-optimization-suite` (implementer must recheck branch and SHA before editing).
- Normative inputs: [CONTRACTS_V1.md](../CONTRACTS_V1.md), Gate-SCHEMA-12 row; [RFC §16.3 and §16.5](../../RFC_FIRST_CLASS_DATA_FORMATS_AND_SCHEMAS.md); [evaluation PRD](../../../LUST_RUNTIME_EVALUATION_PRD_SPEC.md), Phase 3, API-02/03 and SEC-01/03/06.
- Documentation consulted: `repos/lust-lang-docs/src/language-tour/tasks.md`, which describes cooperative create/resume/yield and an explicit round-robin loop. Public runtime documentation belongs in that checkout after implementation; this file is an evaluation/design artifact.

**Decision proposed:** implement generic scoped scheduler services first (DS-114), then `@std/rx` as ordinary library composition (DS-115), with one serialized VM scheduler domain. Do not add Rx AST nodes, keywords, opcodes or implicit threads. Use struct, impl, closures, compile-time `|>`, Effect + Scope. Native host services provide clock/timer/park/wake and accounting through explicit environment/Layer. Observable, operators and terminal policy remain library code. A Rust-only simulation is insufficient gate evidence.

## 2. Static inspection: facts versus required work

Paths below are relative to the compiler checkout, with baseline line anchors. These observations are source facts, not runtime proof.

| Source anchor | Observed implementation | Reuse and gap |
|---|---|---|
| `src/vm/tasks.rs:9–58` | `register_scope_task` registers a child; `close_scope` closes admission, interrupts children, checks terminal task state and `active_scope_tasks`, then finalizes; returns false while joining | Useful cooperative lifecycle foundation. The requested `src/vm/tasks/` directory is actually `src/vm/tasks.rs`. Rx timers, ingress registrations and host futures need explicit join participants; task-state checks alone do not prove their quiescence. |
| `src/vm/tasks.rs:243` | `spawn_task_value` creates and immediately runs | Rx must create suspended, register scope/cleanup, then schedule. Do not spawn first and attach scope afterward. |
| `src/vm/task.rs:102–128`, `src/vm/execution.rs:154` | Task interrupt records first reason; running task unwinds through dispatch poll, nonrunning task can become Stopped immediately; mask defers interruption | Reuse cooperative polls; prove callback/mapper native bridge also polls. Stopped is not evidence that an external producer acknowledged cancellation. |
| `src/effect_abi.rs:257–342` | Scope owns child IDs and LIFO `FnOnce` finalizers; teardown stages exist | `finalize_resources` is callable without internally verifying join. Rx public close must go through the scheduler coordinator, never call it early. Retirement must also bound historical child/finalizer records. |
| `src/effect_abi.rs:355–413` | Fiber context contains identity, state, pending interrupt and mask depth | This struct alone does not carry the entire RFC gas/allocator/environment contract. Wire actual execution/budget context; do not infer containment from its name. |
| `src/effect_abi.rs:416` onward | `ScheduleKind` computes policies; retry/repeat advance state without timer waits | Policy is not Clock/Timer or a park/wake scheduler. |
| `src/effect_abi.rs:614–643` | std timeout spawns a thread, discards JoinHandle, receives with `recv_timeout` | **IN-PATH blocker if used by Rx.** Timeout return does not cancel or join producer. Rx deadlines must use the new scoped protocol; do not use this helper as implementation or proof. |
| `src/effect_abi.rs:670–691` | acquire/release masks acquisition but ignores `register_finalizer` failure | **IN-PATH blocker for subscription acquisition.** Closed-scope registration can fail after acquisition. Require atomic admission/registration or immediate rollback; test it. |
| `src/embed/async_runtime.rs:13–97` | AsyncRegistry holds futures; wake flags coalesce polling | Candidate adapter substrate, no demonstrated scoped cancellation acknowledgement. |
| `src/embed/async_runtime.rs:170` onward | AsyncTaskQueue uses an unrestricted VecDeque, blocking pop; signal future/completer use separate result/waker locks | Not a bounded cancellation-safe Rx ingress. Audit wake registration: completion between empty-result check and storing waker can lose wake. Audit duplicate completion after result has been taken: `Option` emptiness is not a permanent terminal bit. These are source-derived integration risks, requiring RED probes if reused. |
| `src/embed/program.rs:1293–1357` | poll_async_tasks polls registry entries and removes Ready results before resuming/completing VM tasks | Rx adapter needs cancellation removal/ack and stale-generation checks; dropping a future does not establish external-work cancellation. |
| `src/builtins/mod.rs:1266`, `:1825`; `src/vm/stdlib.rs:34`, `:1993` | task and effect builtins exist; task API is create/run/resume/yield/stop/restart | No Rx module registration found in these surfaces. Add library/module resolution and only generic service bridges. Existing API names do not establish typed `@std/rx` availability. |
| `src/vm/stdlib.rs:512–541` | os.sleep blocks the executing thread | Not a cancellable timer; exclude from scheduler implementation. |
| `tests/effect_scope_lifecycle.rs:14` | Existing test manually masks/unmasks a native-future task and checks close/finalizer ordering | Keep as regression; insufficient proof for a live producer or actual suspended callback. |
| `tests/gate_schema_suite.rs:2003` | Gate 12 unconditionally panics | Replace only with production-path assertions. No ignore, catch-panic-to-success, zero-case pass or weakening of other gates. |

## 3. Proposed public library API

The following is **typed contract notation**, not a claim that these declarations compile today. Flash must implement an executable façade using the existing language/module conventions, including packaging the library in distributed artifacts. `Stream<T,E,R>` is an alias for the same cold Observable abstraction, not a second protocol.

```text
Observable<T,E,R>                 // lazy blueprint, fresh state per subscription
Stream<T,E,R> = Observable<T,E,R>
Observer<T,E,Rcb> {
  next: T -> Effect<Unit,E,Rcb>,
  complete: () -> Effect<Unit,Never,Rcb>,
  failure: Cause<E> -> Effect<Unit,Never,Rcb>
}
Subscriber = Observer            // optional naming alias, no extra semantics
Subscription<E>                  // opaque scope/domain/generation handle
BufferOptions { items, bytes, low_items, low_bytes, overflow }
Overflow = Fail | Suspend | DropNewest | DropOldest
RxError<E> = Source(E) | BackpressureOverflow | InvalidOptions | InvalidHandle

from_iter(factory) -> Observable<T,E,{}>
from_producer(start) -> Observable<T,E,{Scheduler}>
interval(period) -> Observable<Tick,Never,{Scheduler,Clock,Timer}>
map(f: T -> U) -> (Observable<T,E,R> -> Observable<U,E,R>)
filter(p: T -> Bool) -> (Observable<T,E,R> -> Observable<T,E,R>)
take(n) -> (Observable<T,E,R> -> Observable<T,E,R>)
map_effect(f, concurrency=1, ordering=Ordered)
  -> (Observable<T,E,R> -> Observable<U,E|E2,R|R2|Scheduler>)
buffer(options) -> (Observable<T,E,R> -> Observable<T,RxError<E>,R|Scheduler>)
subscribe(observer, scope, options) -> Effect<Subscription<RxError<E>>,RxError<E>,R|Rcb|Scheduler>
Subscription.cancel(reason=Cancelled) -> Unit
Subscription.join() -> Effect<Completion<E>,Never,{Scheduler}>
Subscription.close() -> Effect<Completion<E>,Never,{Scheduler}>
Completion<E> { exit: Exit<Unit,E>, suppressed_cleanup_defects }
```

`Never`/type unions above specify semantics; use existing type machinery or explicit library error wrappers rather than introducing syntax. All activation is lazy until subscribe Effect execution. `close` executes cancel then join then release; `join` resolves only after cleanup. `cancel` is nonblocking and idempotent. Cancellation is observed in Completion, never delivered as a recoverable `failure` callback. Ordinary completion/failure has one terminal observer callback; cancellation has zero terminal observer callbacks but exactly one completion outcome. Observer errors feed the same outcome arbiter; an error in a terminal callback becomes a defect, not a second terminal call.

Pure `map/filter` are metered, preserve order and do not perform host effects. `map_effect` initially supports only concurrency=1 and Ordered; reject other settings explicitly until bounded reorder/state accounting is implemented. `take(0)` completes without starting upstream; `take(n)` delivers exactly n then cancels and joins its own upstream child before completing downstream. This internal stop must not change the downstream success into Cancelled. `filter` retains at most one candidate and yields after a bounded poll quantum even when every item is rejected.

`from_producer` receives a library sink with `try_emit(value) -> Accepted | Full(value) | Closed(value)`, `emit(value) -> Effect<Ack,...>` for Suspend, `complete()` and `fail(cause)`. Ownership stays with caller on rejected try_emit. start must register its producer task/cancel adapter before executing. No public untracked callback registration is allowed.

`Subject` is hot, no replay, same-domain only, values before subscription are discarded. Subscriber count is capped. Every subscriber has its own bounded queue; default slow-subscriber overflow terminates only that subscriber with typed Fail, others continue. All-subscriber blocking is unsupported in initial v1. Foreign ingress uses a bounded host bridge and never reenters VM. Explicit drop strategies report counters; default is Fail, never silent dropping.

## 4. Lifecycle, scheduler and cancellation mechanics

### 4.1 Runtime services and registration

Provide injected Scheduler, monotonic Clock and Timer services. Both virtual and physical drivers execute the **same** state machine and dispatch routines. Scheduler offers bounded ready slots, fair run quantum, atomic park-with-recheck, coalesced generation-tagged wake, cancellation wake and join notification. Timer service offers registration plus cancel/unregister acknowledgement. Timers retain stable IDs/generations, not raw closure pointers. Count and remove invalidated timer records; merely making stale entries no-ops still leaks scheduler memory.

Subscription activation is a short masked transaction: validate options and scope admission; reserve control/queue/timer budget; create suspended tasks; register participants and cleanup; publish handle; unmask; schedule. If any step fails, release reservations and acquired resources exactly once, publish nothing, execute no producer. Scope close racing activation either sees the fully registered participant or activation rolls back. No user code executes while this transaction is masked.

Opaque handles validate domain, owner and generation before lookup or capture access. Borrowed callback captures/items cannot escape to asynchronous children without a retained owner; initial async profile accepts owned values only. Synchronous borrowed support is a separate proven profile, not an implicit fallback.

### 4.2 Separate outcome selection from quiescence

Use control state `Active -> Stopping -> Joining -> Releasing -> Closed`, plus one write-once outcome slot and one terminal-callback-dispatched bit. Completion requested while items remain enters a bounded drain phase: no further input; previously accepted values drain before normal complete. Cancel can win until final completion is committed. Failure/overflow immediately stops input and discards queued items, then notifies failure once after the active next callback ends. Repeated complete/fail calls are inert after a committed outcome.

Cancellation linearizes at closing the callback-admission gate and incrementing generation, serialized with callback entry. After this point **no new observer or mapper callback starts**. An already-entered callback may finish or reach an interrupt poll; cancel does not preempt arbitrary native instructions. Counters distinguish callback start from finish. After join returns neither may occur, nor may any producer/timer access subscription resources.

Teardown order:

1. Close enqueue/delivery admission; select Interrupted if cancellation wins; cancel child tokens, timers and ingress registrations; wake parked producer/consumer/mapper.
2. Wait for all actual producer exits, active callback/mapper exits, timer/bridge unregister acknowledgements and dependent children. No lock is held while invoking user code or awaiting acknowledgements.
3. Retire queued events and handles; release queue/mapper/callback captures and registered resources once, in reverse acquisition order. Record cleanup defects while continuing remaining cleanup.
4. Publish Closed and resolve completion/join waiters. Retire scope task/finalizer bookkeeping so repeated subscriptions in one long-lived scope remain bounded.

A cancellation token, a Stopped enum or a dropped Future alone cannot satisfy step 2. Unsupported external producers lacking cancel acknowledgement are rejected on registration. Arbitrary blocking native work has no bounded in-process join guarantee; use an approved contained worker adapter or mark UNSUPPORTED, without timeout-success claims.

### 4.3 Self-unsubscribe, reentrancy and races

Self-unsubscribe calls `cancel`, then returns from callback. Calling/awaiting join or close on the currently executing subscription callback is rejected with a specific `SelfJoin` defect/diagnostic before blocking; destruction is deferred until callback exit. Other tasks may await close. Dropping a handle never bypasses the owning scope's asynchronous teardown obligation.

Nested emit enqueues FIFO into the same bounded mailbox; callbacks are never recursively invoked. `emit(Suspend)` from a callback into its own full queue is rejected as `ReentrantBackpressure`; use nonblocking try_emit there. This avoids waiting on capacity that only the current callback can release.

Deadline comparison is monotonic: successful completion can commit only when `now < deadline`; at equality timeout wins. Virtual ordering is `(time, priority, sequence)` with cancellation/deadline before ordinary ready work at the same time, FIFO within priority. Physical driver checks deadline at outcome commit too, independent of wake scheduling order. Timeout winner requests cancellation, waits for quiescence and cleanup, then resolves Timeout. Completion strictly before deadline remains success; later deadline events are stale. Join itself is not aborted by the cancelled child token; enclosing teardown still owns it.

## 5. Backpressure and memory invariants

Primary cold-stream path is demand-driven pull: do not poll upstream without a downstream slot/credit. Push adapters must reserve admission before queueing. Default overflow is typed Fail. Suspend is opt-in for cooperative sources only; never block the scheduler thread or foreign callback. No unbounded list of blocked senders: cap producer concurrency and pending-send registrations. Foreign ingress returns Full/Closed immediately unless its adapter explicitly supports bounded suspension.

Enforce both `items > 0` and `bytes > 0`, with `0 <= low_items < items`, `0 <= low_bytes < bytes`. High watermark is the configured capacity. Suspend at either bound; resume only after occupancy is at/below both low watermarks and the pending value fits. Capacity check and park registration are atomic with cancellation/drain recheck to prevent lost wakeups. Cancel wakes all blocked sends with Interrupted, never success.

A runtime-owned retained-size charge includes payload, container capacity, metadata and owner-retained allocations; user-provided size callbacks are not trusted accounting. Check arithmetic overflow. Reject a single item larger than bytes before cloning or queue admission. Immutable shared allocations may be conservatively charged per owner; an alias cannot make a large retained allocation invisible. Mapper output must reserve before materialization/publication; independently meter temporary allocations.

Queue caps and total subscription cap are distinct:

```text
queue_items <= C; queue_charged_bytes <= B
retained_total = queue + in_flight + pending_send + operator_state
               + timers + ready_slots + registrations + captured_owners
retained_total <= configured_subscription_budget
```

Single callback/mapper and single pending producer item are each explicitly bounded by max item size; they are not hidden capacity+1 queue entries. Reserve fixed scheduler/control overhead at activation. Apply aggregate scope/runtime caps so many small subscriptions cannot bypass the bound. No accumulating trace logs in production or N/10N memory tests. RSS is a separately measured containment signal, not equal to logical live allocation charges.

## 6. Executable verification specification

Keep the exact entry point `tests/gate_schema_suite.rs::test_gate_schema_12_rx_lifecycle_backpressure`. Replace its panic with calls to shared production-path scenarios; put detailed assertions in proposed `tests/support/rx_gate12.rs` and separately runnable `tests/rx_lifecycle.rs`, `tests/rx_backpressure.rs`, `tests/rx_scheduler.rs`. These filenames are proposed, not existing. The gate must execute all required scenarios, including physical integration or a mandatory referenced test target; running only the exact gate command must not silently omit required coverage.

Fixture: actual VM + guest library pipeline + injected virtual scheduler, with independent host trace recorder, drop sentinels and producer acknowledgements. Tests control scheduling barriers, not task state assignments. Record `next_start/end`, producer pulls/exits, mapper start/exit, timer ack, outcome selection, terminal callback, release and join_return. Test-side expected arrays are literals, never computed by the runtime under test. Separate Rust service tests supplement guest/public-path cases. A bounded recorder asserts overflow rather than dropping evidence.

| ID | Schedule / independent expected oracle |
|---|---|
| RX12-01 scope cancel | Enter a real callback on value 1; park at a cooperative barrier. Cancel externally; offer value 2 and a timer event. No new callback starts. Release callback barrier; expect ordered subsequence `next_start(1), cancel, next_end(1), producer_exit, release, join_return` with timer ack before release. No terminal observer callback; completion Interrupted(Cancelled) once. Advance clock/drain repeatedly: callback counts unchanged, live captures zero. |
| RX12-02 slow consumer | C=2, B=8, fixed charged payloads of 4 bytes (metadata separately reserved), low=(1,4). Pause consumer, accept two; third Suspend parks without being inserted. Consume one: wake once, third admitted, order [1,2,3]. Cancel while another send is parked: it exits Interrupted and producer ack precedes release. Fail policy gives exactly one BackpressureOverflow failure, no third accepted item. |
| RX12-03 dual limits | Independently isolate items using ample bytes and bytes using ample items. Probe cap-1, cap, cap+1; exact cap accepted; cap+1 fails before allocation/copy. Include oversized single item, invalid zero capacity/watermarks and checked-add overflow. Test explicit drop policies and exact drop counters. |
| RX12-04 self-unsubscribe | First next invokes cancel; trace `next_start(1), cancel, next_end(1), release, join_return`, producer_exit before release. Values 2/3 never start. Terminal callback count 0, completion count 1. SelfJoin path fails promptly under watchdog; no deadlock. |
| RX12-05 active upstream | Start real cooperative producer, verify progress >0, park it on timer or capacity, close parent scope. Producer acknowledges exit; no further source pulls, writes, timer registrations or callbacks after join. Masked short acquire delays join and release until unmask. |
| RX12-06 terminal protocol | Success source [1,2]: next [1,2], complete exactly once. Source error after delivered 1: next [1], failure(Fail(e)) once. Duplicate complete, duplicate fail, complete-then-fail and fail-then-complete do not change outcome or callback count. Observer failure/defect and failing terminal callback preserve one outcome; all resources released. |
| RX12-07 virtual deadline | Completion at d-1 succeeds; at d and d+1 times out. Run both insertion orders at equal time and inject cancel/complete permutations with documented ordering. Exactly one outcome; Interrupted has zero recover callbacks. Return happens only after producer/mapper ack and release. |
| RX12-08 late timer | Hold a dispatched timer envelope, cancel+join, reuse handle slot with new generation, deliver old envelope. No capture dereference, no new/old callback, no second outcome. Timer registry and retained captures return to baseline. |
| RX12-09 nested emit | next(1) enqueues 2; expect `start1,end1,start2,end2`, callback stack depth 1. Capacity overflow uses declared policy. Full same-subscription Suspend is rejected, never parks. Reentrant cancellation suppresses queued 2. |
| RX12-10 cancelled mapper | Real map_effect starts and parks; cancel upstream scope; mapper acknowledges interruption; late result is rejected before downstream callback. Map capture destructor after mapper exit; cleanup once. Pure mapper loop is interrupted via actual execution budget/polls. |
| RX12-11 memory scaling | Fixed payload/config, N=10,000 and 10N=100,000 lazy-generated items; retain only counters/hash of output, not all values. Assert same declared upper bound on live charged bytes/items/timers/waiters/ready slots. Repeat subscribe/close in one scope, verify no historical record growth. Separate isolated-process peak RSS report with platform baseline/tolerance; no claim of hard RSS sandbox from logical counters. |
| RX12-12 acquisition / ownership | Close-scope-before-subscribe and close-at-registration barriers produce no start and one rollback. Inject reservation/finalizer registration failure. Borrowed capture escape rejected; owned capture positive control. Stale and wrong-domain handles rejected before dereference. |
| RX12-13 physical driver | Real host timer + real wake/park + real producer cancellation and guest observer. Synchronize using barriers/acks, not sleep-based assumed order. Cancel a blocked send, a waiting timer and a mapper; await production close. Ack and finalizer counters match virtual invariants; late wake is harmless. Run in watchdog child. |
| RX12-14 operators / multicast | Public import + `map`, `filter`, `take` and compile-time pipe: [1..6] → map(*2) → filter(divisible by 4) → take(2) yields [4,8], one complete, joined upstream. take(0) pulls zero. Two cold subscriptions have independent state; Subject fast subscriber continues when slow one overflows, and subscription count cap holds. |

For normal termination, terminal observer callback is counted once; for cancellation, zero. Completion handle resolution is always once after release. Do not assert one user terminal callback on cancellation: that contradicts RFC §16.3.

Physical integration and mutant runs use an external process watchdog with terminate+reap on expiry. Watchdog intervention is test FAIL, not evidence that runtime cancellation worked. Wall-clock tolerance is only a liveness safety ceiling; ordering comes from acknowledgements. Record scheduler seed, complete failure trace, platform, toolchain, configuration and both checkout SHAs. No unbounded native callback is needed to prove cooperative guarantees.

### Required falsification mutants

Each mutant is a minimal production-path patch tested separately against unchanged tests, with a passing unmodified control, recorded diff/hash, exact failing assertion and nonzero exit. Build errors and watchdog-only failures do not substitute for intended oracle failures.

| Mutant | Required rejecting witness |
|---|---|
| Release captures before join | RX12-01/05/10 destructor sees active callback/producer; ordering assertion fails |
| Skip callback admission/generation check | RX12-01/08 observer start count or stale-capture access guard fails |
| Remove item capacity | RX12-02/03 observed queue count exceeds C |
| Remove byte capacity | RX12-03 accepted capacity+1 bytes or preallocation counter changes |
| Unbounded pending-send/ready/timer storage | RX12-11 retained live accounting exceeds fixed bound |
| Remove terminal write-once guard | RX12-06/07 terminal/outcome count exceeds one |
| Cancel token without waking parked producer | Deterministic RX12-05 step-budget assertion fails before external watchdog |
| Resolve timeout immediately without join | RX12-07/13 completion observed before producer ack/release |
| Lose wake during park registration | Forced RX12-02 interleaving exceeds deterministic step budget |

## 7. Actionable handoff to Flash

1. Recheck SHAs, branches and dirty files; preserve unrelated work. Log task start and each implementation handoff in DEVLOG. Confirm this brief's cancellation boundary, equality deadline rule, overflow policy and initial map_effect restriction with coordinator design review.
2. Add failing public-path scenarios and instrumentation contracts first. Keep gate fail-closed until real implementation exists. Record baseline failure command/log; do not create a mock scheduler whose own state transitions are the only oracle.
3. Implement generic scheduler/clock/timer services, bounded registration/wake queues and cancellation-safe admission. Suggested new modules: `src/runtime/scheduler.rs`, `clock.rs`, `timer.rs` (final placement follows existing crate conventions). Keep generic services separate from library operators. Add deterministic lost-wake and timer-retirement tests.
4. Extend `effect_abi.rs`, VM task/scope coordination and the selected embedding adapter so join observes actual participants and acquisition rollback is safe. Retire participants only after actual exit. Do not route through current recv_timeout helper or unrestricted AsyncTaskQueue. If adapting existing signal primitives, first reproduce and fix the identified wake/terminal hazards with regressions.
5. Implement ordinary stdlib Rx structs/impls/closures and typed module exports; verify distributed-package inclusion (Cargo include list currently enumerates `/src/**` and selected files). Add generic builtin bridges only where runtime services require them. No Rx-specific compiler syntax. Use DS-112 callable dispatch; surface unsupported lowering as diagnostics.
6. Implement cold pull, bounded push/suspend, map/filter/take, serial map_effect, then Subject isolation. Apply total-budget accounting to captures, pending work and timers, not only VecDeque length.
7. Wire the exact gate function to all required production scenarios. Execute virtual and physical suites; demonstrate guest-to-VM path and import/pipe behavior. Publish VM/interpreter, JIT bridge/native and AOT coverage separately as PASS/FAIL/UNSUPPORTED/UNVERIFIED; VM success is not native proof.
8. Run mutants separately, then restore clean baseline and run regression suite. Record raw results and mutation evidence under `results/schema/gate12/<revision>/`. Update public docs in `repos/lust-lang-docs` with tested support boundaries; update evaluation status only to the extent proved.

Commands for the implementer, from the compiler checkout (new test targets become valid after step 2):

```sh
cargo test --locked --test gate_schema_suite test_gate_schema_12_rx_lifecycle_backpressure -- --exact --nocapture
cargo test --locked --test rx_lifecycle --test rx_backpressure --test rx_scheduler
cargo test --locked --test effect_scope_lifecycle --test effect_cleanup_and_suspension --test effect_mir_defer_cleanup
cargo test --locked --test gate_schema_suite
```

Run physical/security cases through the external watchdog runner, including when invoked by the exact gate test. Capture actual enabled features and backend configuration; a disabled std test target running zero tests cannot pass. Broaden regression testing only for additional touched components. These commands are a verification plan, not results from this design task.

## 8. Luna acceptance and coordinator reaction

Luna must independently inspect the implementation diff and execute the public-path tests and each required mutant. Require an item-by-item CONFIRMED/PARTIAL/NOT CONFIRMED/UNVERIFIED ledger with source anchors and raw evidence. Reject closure if the gate becomes a Rust-only toy model, callbacks are silenced but producers continue, timer closures linger, join is simulated by setting task state, logical memory accounting omits ownership, or physical integration is missing. Distinguish an unsupported backend from full API support.

All gaps above that affect selected scheduler/adapter paths are IN-PATH under API-02/03 and SEC-01/03/06. Unused embedding helpers may remain documented/deferred only if the shipped Rx path cannot reach them and its own adapter proves the required invariants. General redesign of unrelated gates is outside this handoff. Accepted in-path findings require Luna remediation planning and Flash fixes under repository governance; this design sub-agent launches neither implementation nor review delegates.

**Completion notification — GATE-12-ASTRA-PLAN:** Goal: research and author the Master Implementation and Verification Brief. Artifact: this file; traceability: DEVLOG entry. Evidence: pinned local source/contract inspection and document checks only; no test execution, benchmark or gate closure. **Status: CONCERNS. Coordinator MUST acknowledge receipt and formally respond ACCEPT DESIGN or REQUEST CHANGES, specifically addressing lifecycle linearization, deadline equality, memory accounting and supported backend scope, before closing the design handoff.**
