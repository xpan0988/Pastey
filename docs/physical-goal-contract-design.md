# Physical goal/command contract: design draft

Status: design only, uncommitted draft, 2026-10-03. Nothing here changes code, semantics or tests.

- **Baseline.** Every `file:line` refers to the committed tree at `3376e1e` ("Bound Root-group ledger validation"). The working tree also contains uncommitted TEMP-TRACE hunks, which shift line numbers in `control.rs`, `decision_tools.rs`, `remote.rs`, `core.rs`, `mcp.rs`, `store.rs`, `room_control.rs`, `bridge_lifecycle.rs`, `host_runtime.rs` and `storage.rs`. They are excluded from every statement about current behavior.
- **Unverified.** Anything marked *unverified* was not read in code or documentation during this review.
- **Vocabulary rule.** The contract (§B.1–§B.6) and the watchdog behavior (§C.2–§C.7) use generic terms only. ROS-specific names (Nav2, rosbridge, ros2_control, action and message types) appear only in the adapter material, §B.7 and §D.6. Elsewhere "ROS" appears only as the name of an adapter.
- **Owner re-scope (input).**
  - Pastey keeps: discovery, authority and review, capability exposure, agent→capability invocation, cross-Host routing, result correlation, the high-level journal, Burn/revoke intent, and witness independence.
  - The device side owns HOW: servo watchdog, device lifecycle, movement deadline enforcement, wall-clock physical validity, collision safety, joint limits, controller health, the per-tick consequence ledger, and the freshness control loop.
  - Hard constraints: ROS is not a premise of Core, and a thin lease watchdog runs on the binding side, next to the robot.

Legend: **KEEP** = keep in Core, **MOVE** = move to the binding, **THIN** = demote to the thin binding-side watchdog, **DELETE**, **UNDECIDED**. ⏱ = the mechanism exists only because of the 1 s action / 200 ms observation tick.

---

## Part A: Subsystem inventory

### A.0 Facts shared by every row

- **Topology.** Brain Host ⇄ Bridge ⇄ executor Host ⇄ (binding) ⇄ device. Admission, evidence and consequence run on the executor (`docs/physical.md:11`; `decision_tools.rs:1-4`).
- **The Core lock.** Each Host has one `parking_lot::Mutex<PhysicalControlServiceV1>` (`host_runtime.rs:8`, `host_runtime.rs:71`). Every `&mut self` Core method runs under it.
  - The ledger has its own connection mutex (`store.rs:1086-1119`), taken inside the Core lock. Every store call opens a transaction and starts with `audit()` (`store.rs:992-1021`). A full audit runs under the Core lock (`docs/physical.md:252`).
- **Binding I/O.**
  - Async binding I/O releases the Core lock and takes it again: install `control.rs:575`/`583`, apply `control.rs:1070-1081`, fence `control.rs:1161-1175`.
  - `observe()` runs in `spawn_blocking` without the lock (`decision_tools.rs:391`). Its ingestion is under the lock (`decision_tools.rs:397-405`).
- **Witnesses** run under the Core lock and inside an IMMEDIATE ledger transaction (`core_evidence.rs:103-115` → `store_evidence.rs:466-478`).
- **Route validity** is re-read on every Root validation, under the Core lock (`core.rs:61-63`, `core.rs:615-623` → `bridge_lifecycle.rs:85-110`). The check reads the app database and fails when the peer is not `Connected` (`bridge_lifecycle.rs:98-107`).
- **Supervisor cadence** is `max_gap/2` (`decision_tools.rs:448-450`). That is 200 ms in the demo (`physical_demo_tests.rs:108-109`) and in the dev envelope (`bindings/dev.rs:44-56`, `max_gap_us: 400_000`, `action_us: 1_000_000`, `min_decision_interval_us: 200_000`).
- **Production bindings.** None is compiled in. The witness registry is empty in production (`adapters/host_bindings.rs:1-15`). The R1–R23 rules are implemented only by the simulated runtime (`bindings/sim.rs:1-10`).

### A.1 Inventory

Line counts are non-test lines (≈). "Lock" means the subsystem runs under the Core lock.

| # | Subsystem | Where (HEAD) | LoC | Main deps | Lock | Owning concern | Class | Reason | ⏱ |
|---|---|---|---|---|---|---|---|---|---|
| 1 | Claims, values, descriptors, `BoundSetV1` | `values.rs` (253), `descriptor.rs:1-628`, `contracts.rs` (610) | ~1,490 | serde, digest | data only | WHERE/authority: envelope language | **KEEP** | Capability exposure and envelope. **Gap:** no code checks a payload against a `BoundSetV1`. Bounds are only narrowed (`descriptor.rs:457-534`). Admission checks option digests (`docs/physical.md:21`). Goals with parameters need a new check. | Partly: `ObservationFreshnessV1` and `ProposalFreshnessV1` (`contracts.rs:142-193`); decision and observation rates (`contracts.rs:9-13`, `contracts.rs:75-86`) |
| 2 | Review → approval → Root → grant basis; executor policy; narrowing | `core.rs:204-865`, `store_core.rs` (661) | ~1,300 | store, resolver | yes | authority | **KEEP** | Exact digest, one approval one Root (`core.rs:482-485`, `core.rs:586`), narrowing only (`core.rs:792-865`). This is the authority core. | no |
| 3 | Ledger infrastructure: schema, staged DDL, journal/trust, full and bounded audit, write kinds | `store.rs:1-1738`, `store_native.rs`, `store_qualification.rs` | ~1,800 | rusqlite | yes, plus connection mutex | high-level journal | **KEEP** (shrinks) | The journal stays. Bounded validation (`3376e1e`) exists because Root history grows per tick (`docs/physical.md:252-265`). | indirect |
| 4 | Control ledger: sessions, reservations, budgets, actions, decisions, write callbacks, effect violations | `store_control.rs` (1,291) | ~1,290 | rusqlite | yes | authority plus result correlation | **KEEP** | An action row becomes a goal row. The callback tables are reusable for late dispatch results. | no |
| 5 | Evidence ledger: evidence, consequence revisions, reconciliation, acceptance, handover | `store_evidence.rs` (1,368) | ~1,370 | evidence.rs | yes | outcome correlation | **KEEP** acceptance, reconciliation, handover; **DELETE** per-tick appends | Every evaluation inserts a consequence revision that lists every observation id (`store_evidence.rs:466-519`, list at `:493`). This is the quadratic growth in `docs/physical.md:257-265`. | yes |
| 6 | Session reservation, conflict domains, epochs | `control.rs:415-500`, `store_control.rs:349-430`, `binding.rs:588-654`, `store.rs:1227-1272` | ~330 | resolver, store | yes | authority: who holds the body | **KEEP** | Exclusive holding with monotonic epochs (`control.rs:437-444`; `close_root` bumps epochs at `store_control.rs:650-662`). The epoch is also handed to the watchdog (R4, R16). | no |
| 7 | Identity and incarnation checks | resolve `binding.rs:457-551`; per sample `control.rs:682-689`; lineage `evidence.rs:19-40`; reset closes the environment `core_evidence.rs:55-74` | ~200 | — | yes | authority (identity) / HOW | **KEEP** at resolve, goal and witness level; **DELETE** the per-sample check | Body replacement must still invalidate. Per-sample re-checking is part of the freshness loop. | per-sample part |
| 8 | Session lease | deadline `control.rs:422-436`; check `control.rs:521-522`; view `control.rs:249-252`; enforced by the sim (`sim.rs:380-417`, `sim.rs:444-451`) | ~60 | clock | yes | lifecycle of the grant (Pastey) / enforcement (device) | **THIN** | Core computes and records the lease. Only something next to the body can enforce it without Core. | no |
| 9 | Tool dispatcher: tool sessions, commit/release, decide, refusal records, observe filtering | `decision_tools.rs:1-381`, `decision_tools.rs:615-707` | ~560 | control, store | yes, per call | agent→capability invocation | **KEEP**, as the goal dispatcher | Brains are replaceable through one generic tool surface. | Partly: `decide` samples before every decision (`decision_tools.rs:638-641`); observe rate (`decision_tools.rs:359-367`) |
| 10 | Supervisor: `stream_tick`, `supervise_stream`, `TickTimerV1` | `decision_tools.rs:64-110`, `decision_tools.rs:434-521` | ~130 | tokio timers | 3–4 lock acquisitions per tick (`decision_tools.rs:452`, `:397`, `:468`, `:473`) | mixed: idle lease and budget (grant) plus sampling (HOW) | **DELETE**, replaced by event-driven revoke and one-shot timers | This is the cross-device 200 ms Core supervisor that the owner wants gone. | **yes** |
| 11 | Observation and sample ingestion: `TrustedControlObservationV1`, `ObservationValidityV1`, `record_control_observation`, `issue_proposal_challenge`, `continuing_deadline`, `ingest_binding_sample` | `control.rs:131-152`, `control.rs:217-222`, `control.rs:353-374`, `control.rs:667-770`, `control.rs:1683-1707`; challenge and observation coupling in admission `control.rs:863-881`, `control.rs:940` | ~210 | clock | yes | HOW: the hardware freshness loop | **DELETE** | `max_age`/`max_gap` gates (`control.rs:694-708`); a fresh fact extends `continuing_deadline` (`control.rs:709-726`). Goal feasibility is the device's. | **yes** |
| 12 | Admission: option declared and approved, digest, rate, duration, lifetime ≤ lease and Root, cumulative budget, sequence | `control.rs:784-951`, `store_control.rs:431-474` | ~210 | store | yes | authority | **KEEP**, as goal admission | Add a parameter-in-`BoundSetV1` check (row 1). Drop the challenge and observation coupling (⏱). | challenge part |
| 13 | Dispatch and write-result recording: `dispatch_decision` CAS, `prepare_write`, `apply`, `finish_write`, `physical_write_callbacks`, lapsed vs revoked | `control.rs:952-1155`, `store_control.rs:475-579`, `store_control.rs:82-132`, `store_control.rs:1139-1244` | ~450 | store, clock | prepare and record under lock; apply outside | result correlation | **KEEP** | One dispatch per goal (`control.rs:1022-1027`), no retry. Exact answer recorded apart from "may it still run" (`control.rs:1085-1094`). | the observation-freshness half of validity (`control.rs:991-994`) |
| 14 | Replacement fence: a new decision closes the previous one | `control.rs:909-921`, `store_control.rs:458-460` | ~20 | — | yes | HOW: preemption | **UNDECIDED** | At goal granularity, whether a new goal preempts the old one is device semantics. Core keeps "at most one open goal per grant" as a record rule. | yes (1 s slices) |
| 15 | Fence, revoke, quarantine, latch, NativeFence placeholder | `control.rs:1156-1191`, `store_control.rs:597-678`, `binding.rs:624-654`; NativeFence fail-closed `control.rs:594-597`, `control.rs:1103-1106`, `control.rs:1183-1186`, `store_native.rs` | ~200 | binding | close and ack under lock; `fence()` outside | Burn/revoke intent (Pastey) / stop (device) | **KEEP** revoke intent, the RAM→ledger→fence order and the quarantine record; **THIN** latch and cancel; NativeFence **UNDECIDED** | NativeFence has no producer anywhere (`docs/device-binding-protocol.md:122-124`). | no |
| 16 | Loss profile | `contracts.rs:394`, equality at `contracts.rs:454-457`; `loss_digest` `control.rs:91`, `control.rs:638`; sim `sim.rs:47`, `sim.rs:110-112` | ~20 | — | data | HOW | **MOVE**: becomes the declared safety mechanisms (§B.5) | Core already only compares it by equality. It never evaluates it. | no |
| 17 | Witness registry, classes, verdict admission | `evidence.rs:329-384`; classes `contracts.rs:337-363`; registry checks `core.rs:169`, `core.rs:218-229`, `core.rs:970-980`, `store_evidence.rs:240-287` | ~250 | — | yes | witness independence | **KEEP** | `NativeSelfReport` never satisfies a requirement (`contracts.rs:358-362`). | no |
| 18 | Series checks: window, continuity, gap, dwell, freshness over the full per-tick series | `evidence.rs:472-536`, `evidence.rs:539-596`, `evidence.rs:674-747`, `evidence.rs:809-890` | ~330 | — | yes | outcome correlation (Pastey) over HOW data | **THIN** to a bounded summary (§B.4) | Recomputing over thousands of samples is the per-tick consequence ledger. | yes |
| 19 | Per-tick consequence evaluation | `decision_tools.rs:579-614`, `core_evidence.rs:103-115`, `store_evidence.rs:466-519` | ~110 | witness | yes, inside an IMMEDIATE transaction | outcome correlation | **DELETE** per tick; **KEEP** one evaluation per completion event | — | yes |
| 20 | Effect bound checked each tick | `decision_tools.rs:522-578`; scope `contracts.rs:53-60` | ~60 | witness | yes | HOW (geofence) presented as authority | **THIN**: an end-of-goal witness check plus a device-declared keep-out mechanism | Today Core acts as a runtime geofence. See A.5 #6. | yes |
| 21 | Cumulative budgets: count and time | `contracts.rs:195-218`; `store_control.rs:20`, `:447`, `:495`, `:671`, `:874-883` | ~80 | store | yes | authority | **KEEP** | Never replenished (`docs/device-binding-protocol.md:89`). | no |
| 22 | Acceptance (L7), reconciliation, status projection | `core_evidence.rs:116-184`, `store_evidence.rs:521-585`, `store_evidence.rs:616-807`, `store_remote.rs:262-362`, `protocol.rs:182-258` | ~550 | witness, store | yes | outcome correlation | **KEEP**, simplified | ack ≠ consequence ≠ acceptance (`docs/physical.md:178`). | no |
| 23 | Handover: a witness-verified safe state releases quarantined domains | `evidence.rs:436-463`, `evidence.rs:848-890`; `store_evidence.rs:616-807`; tables `store_evidence.rs:12-55` | ~250 | witness | yes | authority (re-grant) over a HOW predicate | **KEEP**, reduced to one post-cancel verdict; **UNDECIDED** whether re-grant is gated on it | See A.5 #7. | trace-based part |
| 24 | Remote protocol and cross-Host routing: `physical-control-v2`, semantic idempotency, Start/Cancel/Status/Reconcile/Tool* | `remote.rs` (949), `protocol.rs` (258), `store_remote.rs` (471); `room_control.rs:4282-4400`; `bridge_lifecycle.rs:85-110` | ~1,700 | Room Control | yes (`room_control.rs:4322-4326`) | cross-Host routing and correlation | **KEEP** | Two fixes needed. Route loss is detected by polling (row 10). Burn closes Roots without a fence (A.5 #1). | partly (polling) |
| 25 | Bridge flow control and compatibility cache (`a193eda`) | `room_control.rs:3173-3192`, `host_runtime.rs:1141-1185` | ~150 | — | no | routing | **KEEP** | — | no |
| 26 | MCP relay (`--physical-mcp`) | `mcp.rs` (555), `host_runtime.rs:1186-1290`, `main.rs:132-137` | ~700 | Bridge relay | brain Host's lock on each 20 ms poll (`mcp.rs:62-63`, `mcp.rs:86-104`, `host_runtime.rs:1233-1240`) | agent→capability invocation | **KEEP** | Tool definitions change to goal kinds with parameters plus `cancel`. The polling is transport-level (`docs/physical.md:270`), not body-tick. | no (transport) |
| 27 | `EnvironmentBinding` trait and views | `control.rs:154-402` | ~250 | — | — | device seam | **KEEP**, changed (§B.6) | — | `observe`, `BindingSampleV1.control`, `continuing` |
| 28 | Binding resolver: enrollment, resolution, qualification, fingerprint, clock | `binding.rs` (non-test ~700), `store.rs:1121-1347` | ~930 | store | yes | discovery and authority | **KEEP**, plus safety declarations | Fingerprint equality (`contracts.rs:287-317`). Self-described means simulation only (`binding.rs:483-487`, `binding.rs:902-908`). Clock regression closes the resolver (`binding.rs:376-393`). | no |
| 29 | Device-binding protocol R1–R23 | `docs/device-binding-protocol.md:36-83`, `sim.rs:367-602` | doc + ~240 | — | — | HOW | see A.3 | — | R11, R13 (refresh), R15, R17, R19a |
| 30 | Dev-only `physical-sim` bindings | `bindings/` (sim 831, flat 261, dispenser 147, dev 183), `adapters/host_bindings.rs` (58), `mod.rs:53-58` | ~1,490 | Core trait | — | HOW (reference device) | **MOVE**: already binding-side; rewrite as a goal sim plus a watchdog sim | Release builds refuse the feature (`mod.rs:55-58`). | yes (STEP and lazy integration are fine; the 1 s options are ⏱) |
| 31 | Generic Managed Worker path | `managed_*.rs`, `worker_*.rs` | n/a | — | — | not Physical | n/a, unchanged | No `crate::physical` reference outside the callers listed below. "Physical" there means filesystem paths (`managed_objects.rs:1-4`). It shares only `purge_room` (`host_runtime.rs:272-290`). | n/a |

The only callers of `crate::physical` outside the module are `commands.rs` (Tauri commands `:7461-7490`), `host_runtime.rs` (Core construction `:189-205`, purge `:280-282`, MCP relay `:1186-1290`), `room_control.rs` (ingress `:1559-1601`, `:4282-4400`), `peer_capabilities.rs:206-211`/`:361-367`, `storage.rs:195` and `main.rs:39`/`:134-136`.

### A.2 Tick-specific mechanisms

Each of these exists only because decisions are ≤1 s slices and the Core samples the body every 200 ms. They have no meaning at goal granularity:

1. Supervisor loop and absolute-deadline tick schedule (`decision_tools.rs:493-521`, `decision_tools.rs:106-110`).
2. Control observation freshness `max_age_us`/`max_gap_us` and identity replay (`control.rs:690-708`).
3. `continuing_deadline`: a fresh fact maintains a live action (`control.rs:98`, `control.rs:709-726`, `control.rs:940`, `control.rs:991-994`).
4. A proposal challenge bound to the latest observation (`control.rs:738-770`, `control.rs:863-881`).
5. A fresh sample before every decision (`decision_tools.rs:638-641`).
6. Decision rate `min_decision_interval_us` at 200 ms granularity (`control.rs:857-862`). A goal rate remains, but as a coarse anti-flood rule.
7. Replacement fence between consecutive decisions (`control.rs:909-921`).
8. Per-tick evidence append plus a consequence revision listing every observation (`store_evidence.rs:466-519`), and the replay and bounded-validation machinery that keeps it affordable (`store.rs:297-358`, `store.rs:1371-1569`; `docs/physical.md:183-265`).
9. Per-tick effect-bound witness over all of an action's evidence (`decision_tools.rs:522-578`).
10. Series continuity, gap and dwell admission (`evidence.rs:472-536`, `evidence.rs:674-747`). Dwell-based completion is still needed, but the witness computes it, not Core.
11. `status()` on every sample (`control.rs:1695`) and on every tool-session open (`decision_tools.rs:226`): controller-health polling.
12. Route validity polled on every authority check (`core.rs:61-63`), which is why `c8556b0` had to make it read-only.
13. Device protocol R11 (sequenced refresh), R13's refresh-loss interval and R15 (control loop never waits on IPC) (`docs/device-binding-protocol.md:61-65`).

### A.3 R1–R23 at goal granularity

"Survives" means it stays a requirement in the goal contract (§B). "Watchdog" means it becomes a rule of the binding-side watchdog (§C). "Device" means it becomes the device stack's own business and leaves the Pastey contract.

| Rule (`docs/device-binding-protocol.md`) | Today | Goal granularity |
|---|---|---|
| R1 fresh incarnation, no restored task state (`:42`) | sim `launch` (`sim.rs:219-306`) | **Watchdog.** A new watchdog process has a fresh incarnation, restores no lease, and cancels any device goal it finds at start (§C.3). |
| R2 identity fixed at configuration (`:43`) | sim `install` (`sim.rs:430-436`) | **Survives** (describe and qualification). The watchdog rejects a lease for a foreign body or domain. |
| R3 replacement ⇒ new incarnation (`:44`) | lineage checks | **Survives.** Goal results and witness verdicts bound to an old incarnation are rejected. |
| R4 monotonic high-water epoch per domain (`:48`) | sim `sim.rs:444-451` | **Watchdog:** lease epoch. Core keeps the durable epoch ledger. |
| R5 exact duplicate install is idempotent, no extension (`:49`) | sim `sim.rs:438-443` | **Watchdog:** lease install idempotent; renewal only by an explicit `Renew` (§C.4). |
| R6 newer install supersedes and drops the action (`:50`) | sim `sim.rs:452-455` | **Watchdog:** a newer lease cancels the old lease's goal first. |
| R7 one connection owns the session (`:51`) | — | **Watchdog:** one lease holder (the binding connection). |
| R8 lease in the future and ≤ max lease (`:52`) | sim `sim.rs:444-451` | **Watchdog**, with `max_lease_us` declared in qualification (§B.5). |
| R9 no command before admission (`:56`) | sim `sim.rs:477-489` | **Survives:** no goal reaches the device without a valid lease and a Core-admitted goal id. |
| R10 one exact action identity, digest and deadline within lease (`:57`) | sim `sim.rs:490-496` | **Survives:** goal id, parameter digest and goal budget ≤ lease remaining. A changed goal under the same id is rejected. |
| R11 commands match exactly, strictly increasing sequence, refresh never extends (`:58`) | — | **Disappears** ⏱. There are no refresh commands: the device owns execution. |
| R12 invalid command from the owner closes its window (`:59`) | — | **Watchdog:** a malformed or mismatched goal from the holder is refused. A lease or epoch mismatch is ignored and inert. |
| R13 self-stop at the action deadline, lease deadline or refresh loss (`:63`) | sim `catch_up` (`sim.rs:380-417`) | **Watchdog:** cancel at the goal budget or on lease loss. The refresh-loss interval disappears ⏱. The lower-level stop is the device's declared mechanism (§B.5). |
| R14 a closed action never resumes (`:64`) | sim `sim.rs:490-499` | **Survives:** a goal id is never dispatched twice, and the latch holds (§C.3). |
| R15 control loop never waits on IPC (`:65`) | — | **Device.** The watchdog's loop is local, but the control loop is not Pastey's. |
| R16 fence names install and next epoch; advance high-water (`:69`) | sim `apply_fence` (`sim.rs:510-552`) | **Watchdog:** `Revoke(lease, next_epoch)` ⇒ cancel and latch. |
| R17 fence serializes with actuation (`:70`) | sim `sim.rs:537-548` | **Split.** Watchdog: revoke serializes with goal dispatch in one state machine. Device: "no actuation after cancel" becomes the device's cancel semantics. Pastey records ack, not stop. |
| R18 fence racing refresh leaves the action closed (`:71`) | — | **Watchdog:** revoke racing dispatch leaves the goal refused or cancelled. |
| R19 delayed commands rejected after fence; delayed ack inert (`:72`) | sim | **Watchdog:** latched leases reject everything. |
| R19a sealed evidence after fence; `fenced` disposition (`:73`) | sim `sim.rs:540-548`, `sim.rs:557-602` | **Thin:** one post-cancel snapshot and verdict for handover, not a continuous trace ⏱. |
| R20 loss of the owning connection closes the window; no resume (`:77`) | — | **Watchdog:** link loss ⇒ cancel and latch; reconnect needs a new lease. |
| R21 controller or I/O loss latches until a fresh process (`:78`) | sim `status()` (`sim.rs:634-636`) | **Watchdog:** cancel failure or device fault ⇒ `Faulted` latch until restart. Controller health is the device's. |
| R22 unknown fields fail to parse; partial frames inert (`:82`) | — | **Survives:** wire hygiene of the watchdog protocol. |
| R23 replies report identity, request, install, sequence, epoch… (`:83`) | — | **Survives, simplified:** `lease_id`, epoch, `goal_id`, `op_id`, state and incarnation. Command-sequence fields are dropped ⏱. |

### A.4 Minimal Core invariants for the five acceptance criteria

The acceptance criteria are in `tests/physical_demo/README.md:27-89`.

| # | Invariant | Criteria | Existing anchor |
|---|---|---|---|
| I1 | Authority is Core-minted, finite and exact. A sealed digest is reviewed, one approval makes one Root, and the scope can only narrow. Expiry is checked on the monotonic clock and recorded in wall time. | 1, 2 | `core.rs:461-611`, `core.rs:792-865` |
| I2 | Every goal passes Core admission against the envelope before any device sees it. Checks: kind approved and declared; parameters inside `BoundSetV1`; goal budget ≤ per-goal ceiling, lease and Root; cumulative count and time; coarse rate. It is recorded with its proposer, allowed or refused. | 2, 4 | `control.rs:823-951`, `store_control.rs:431-474`, `store_control.rs:691-803` |
| I3 | One dispatch per admitted goal, no retry. A missing or mismatched answer is `unknown`. Budgets are consumed at dispatch and never replenished. | 2, 3 | `control.rs:1022-1027`, `store_control.rs:475-498`, `docs/device-binding-protocol.md:89-90` |
| I4 | Revocation linearization: RAM closes, then the ledger, then the revoke is pushed to the watchdog. Closed Root, lease and goal ids are never reused, and nothing resumes without a new approval. | 3 | `control.rs:1161-1169`, `core.rs:729-734`, `docs/physical.md:177` |
| I5 | Every dispatched goal is covered by a lease that the watchdog enforces locally on its own monotonic clock without Core. Lease loss, revoke or link loss ⇒ cancel and latch. | 3 | new (§C) |
| I6 | Separate records for proposer and admission, dispatch answer, device completion claim, witness verdict and acceptance. Verified only from an admitted verdict of the required class; `NativeSelfReport` never suffices. | 4 | `contracts.rs:337-363`, `evidence.rs:539-596`, `store_evidence.rs:521-585` |
| I7 | Core is device-agnostic: opaque ids, digests, bounds and classes only. A new body needs no Core change, and `check:core-agnostic` stays 0. | 1, 5 | `scripts/check-core-device-agnostic.sh` |
| I8 | Fail-closed. Unknown denies. A missing safety declaration refuses start. A missing witness leaves the goal unverified. | 2–5 | `docs/physical.md:170-173` |
| I9 | Exclusive domain holding with monotonic epochs: one lease per conflict domain, stale epochs rejected, quarantine until the release rule is met. | 3, 5 | `control.rs:437-474`, `store_control.rs:639-678` |

Not needed for any criterion:

- per-observation freshness, challenges and continuing deadlines;
- the supervisor tick;
- per-tick consequence revisions;
- series replay in the ledger audit;
- a per-tick effect bound;
- NativeFence.

### A.5 Findings against the keep/abandon list

1. **Burn and Bridge loss reach the device only through the 200 ms supervisor.**
   - `purge_room` → `invalidate_physical_bridge` → `invalidate_physical_peer` → `close_root` closes RAM and ledger. It requests no fence (`host_runtime.rs:272-282`, `remote.rs:163-205`, `core.rs:729-734`).
   - The fence is sent only when the stream's next `stream_tick` finds the session invalid and calls `end_stream` → `revoke_control_session` (`decision_tools.rs:451-464`, `decision_tools.rs:417-433`, `control.rs:1156-1191`).
   - A probe failure marks the peer `reconnecting` without notifying Physical at all (`bridge_lifecycle.rs:289-322`). Physical learns of it by polling (`core.rs:61-63`).
   - Consequence: deleting the supervisor (the abandon list) without adding an event-driven revoke push would leave Burn, a kept item, with no device-side effect until the running action's own deadline.
2. **Wall-clock time still decides physical validity.**
   - Evidence capture and receipt times are wall-clock microseconds (`core_evidence.rs:32-54`; sim `now()` multiplies wall ms by 1000, `sim.rs:329-336`).
   - Completion admission judges `stale_observation` from them (`evidence.rs:719-721`).
   - Grant expiry in wall time (`core.rs:530-545`, `core.rs:636-639`) is authority and legitimately Pastey's. Evidence freshness in wall time is "wall-clock physical validity", which the list abandons.
3. **The freshness control loop is in Core** (`control.rs:667-737`, the `continuing_deadline` horizon `control.rs:709-726`). It is on the abandon list. The standing constraint forbids changing `max_age`/`max_gap` now, so this document proposes removal only for a new goal mode (§D.3), never as an edit to DecisionStream.
4. **Movement deadline enforcement is already mostly the device's.**
   - The body stops at its own action or lease deadline in the binding (`sim.rs:380-417`).
   - Core's deadline checks are admission and recording gates (`control.rs:882-886`, `control.rs:959-1009`, `control.rs:161-171`). This is consistent with the list.
   - However, `LaneValidityV1.allows()` (`control.rs:161-171`) asks the binding to re-check Core's monotonic deadline before each native write (`docs/device-binding-protocol.md:32`). That is a cross-device validity handle, and it disappears in the goal contract.
5. **Witness independence is enforced by declared class only.** The only existing witness is the simulation oracle inside the same binding that produces the evidence (`sim.rs:675-831`, `sim.rs:557-602`). Core trusts the registry's class (`core.rs:970-980`); `IndependentMeasured` has no implementation. This is consistent with the keep list, but it is a gap for hardware.
6. **Core runs a runtime geofence.** `check_effect_bound` evaluates the witness every tick and closes the Root on a contradiction (`decision_tools.rs:522-578`). In practice that is Core-side safety supervision of the body, in tension with "Pastey never claims safety" and with "collision safety → device". Proposed: an end-of-goal verdict (audit) plus a device-declared keep-out mechanism (§B.5).
7. **Device lifecycle is partly in Core.**
   - Session states `installing/active/quarantined` (`store_control.rs:9-18`) are grant lifecycle. Keep them.
   - Domain release requires a witness-verified "at rest" trace after a `fenced` disposition (`store_evidence.rs:12-20`, `store_evidence.rs:616-807`). That is device safe-state knowledge gating re-grant. Proposed: keep only the gate (one post-cancel verdict), and let the binding define "at rest". UNDECIDED.
8. **Controller health is polled by Core.** `status()` runs on every sample (`control.rs:1695`) and every use of the sealed binding (`binding.rs:552-560` via `producer_check`). This is on the abandon list. Demote it to "checked at dispatch and on binding events".
9. **No collision or joint-limit logic exists in Core.** Consistent with the list.
10. **Generic Managed Worker:** no overlap with Physical (A.1 row 31).

---

## Part B: Generic goal/command contract

### B.1 Terms

- **Goal.** One named, parameterized intent of a declared kind, executed by the device on its own until it ends with a completion event or a cancel. Example: "go to target T within 40 s at most effort E".
- **Command.** A goal whose execution is short and fully device-owned, such as "sit" or "pour 20 ml". It uses the same contract.
- **Lease.** The binding-side representation of a Pastey grant. While a lease is valid the watchdog lets goals run; when it is lost the watchdog cancels and latches.
- **Envelope.** The approved, narrowable set of goal kinds, parameter bounds, per-goal and cumulative budgets, rate, lifetimes and lease parameters.

### B.2 Authority envelope

This is a new invocation mode next to `DecisionStream` (`descriptor.rs:550-555`). It reuses `ReviewScopeFieldsV1`'s identity, qualification and budget fields (`contracts.rs:377-395`).

```text
GoalScopeV1 {
  kinds:            [GoalKindGrantV1]        // approved subset of declared kinds
  max_goal_us:      PositiveMicros           // per-goal time budget (device must finish or be cancelled)
  min_goal_interval_us: PositiveMicros       // coarse anti-flood rate, not a control rate
  execution:        ExecutionBudgetV1        // reused: action_count = total goals, total_execution_us
  lease:            { duration_us, renew_interval_us }   // ≤ declared watchdog maxima (§B.5)
  idle_lease_us, approval_lifetime_us        // reused from DecisionStreamScopeV1 (contracts.rs:75-86)
  observation:      ObservationFlowV1        // reused (contracts.rs:9-13): what `observe` may release
  completion:       { predicate: ContractRefV1, required_witness: WitnessClassV1, verdict_within_us }
  envelope_claim:   target_only | path_contained   // what Review may say; see "Not guaranteed"
}
GoalKindGrantV1 { kind: LabelV1, params: BoundSetV1 }   // BoundSetV1 reused (descriptor.rs:457-534)
```

How each envelope element is expressed, all through `BoundSetV1` dimensions. Core sees pointers and kinds only, never meaning.

| Element | Expression | Checked by |
|---|---|---|
| Region or waypoint set | `Enum` on a target-name pointer (named waypoints the binding resolves), or `Interval`s on target coordinate pointers (an axis-aligned box in the binding's frame) | Core at admission (new `BoundSetV1::admits(payload)`, fail-closed for unbounded leaves as `descriptor.rs:457-459` intends) |
| Time budget | `max_goal_us`, a goal's requested budget ≤ min(`max_goal_us`, lease remaining, Root remaining, cumulative remaining) | Core at admission; the watchdog enforces it locally |
| Speed or effort cap | `AbsMax` on an effort pointer that every goal must carry | Core checks the value; the device enforces it through a declared mechanism covering `overspeed` (§B.5) |
| Cumulative budget | `execution.action_count` (goals), `total_execution_us` (sum of granted goal budgets, reserved at admission and consumed at dispatch, as today `store_control.rs:447`, `store_control.rs:495`) | Core |
| Expiry | approval and Root lifetime (`core.rs:530-551`) | Core, plus the watchdog through the lease |

**"A bad brain cannot leave the envelope" at goal level** means four things:

1. Core refuses any goal whose kind, parameters, effort cap, budget or rate is outside the envelope, and records the refusal (I2).
2. The watchdog cancels every goal at its budget or on lease loss, without Core (I5).
3. The device enforces declared caps. Pastey records the declaration and does not verify it.
4. The witness judges the end state after the goal, and the path summary when `path_contained` is claimed. A contradiction closes the Root, and a new grant needs a new review.

**Not guaranteed any more, stated plainly:**

- **The trajectory between start and target.** The device's planner chooses it. A goal with an in-region target can transit outside the region unless the device declares a mechanism covering `out_of_region`. Even then it is the device's guarantee, not Pastey's.
- Speed profile, acceleration, collision avoidance, time-to-stop after cancel, joint and controller limits.
- **Fine intra-goal control.** Today a brain's authority is re-checked every ≤1 s slice against a fresh observation (`control.rs:738-951`). After the re-scope, one admitted goal grants the device up to `max_goal_us` of autonomous motion toward an approved target. Containment drops from "per slice, freshly observed" to "per goal: admitted target, time-bounded, verified afterwards".
- Review must show `target_only` unless `path_contained` is backed by a declared device mechanism (§B.5 rule 4).

### B.3 Messages and lifecycle

```text
describe → qualify → [review/approve: unchanged] → start(lease install)
  → dispatch(goal) → feedback* → completion event
  → (next dispatch …) | cancel(goal) | revoke(lease)
```

| Message | Direction | Fields | Notes |
|---|---|---|---|
| `Describe` | Core → binding | — | The reply adds `safety_declaration` (§B.5) and `goal_kinds` (kind name, parameter schema digest, declared `BoundSetV1`) to today's `BindingDescriptionV1` (`binding.rs:213-219`). |
| `Qualify` | Core | profile, qualification, safety declaration digest | The refusal rule is in §B.5. |
| `LeaseInstall` | Core → binding → watchdog | `lease_id`, `epochs`, `duration_us` (relative), `max_goal_us`, `renew_interval_us`, `scope_digest`, `safety_digest` | Reply: `LeaseInstalled{lease_id, epochs, watchdog_incarnation, remaining_us}`, or `Refused{reason}`, or no reply ⇒ unknown, Root closed (as `install_control_session`, `control.rs:570-624`). |
| `Renew` | Core → watchdog | `lease_id`, `epochs`, `seq`, `extend_us` (relative, capped by Root remaining) | Coarse cadence, see §C.4. Reply: `Renewed{seq, remaining_us}`. |
| `Dispatch` | Core → binding → watchdog → device | `lease_id`, `epochs`, `goal_id`, `op_id`, `kind`, `params` (canonical JSON), `params_digest`, `goal_budget_us` (relative) | Reply: `accepted` or `refused`, plus an opaque `device_handle_digest`. No reply or a mismatch ⇒ `unknown`. Exactly one per `goal_id` (`dispatch_decision` CAS reused, `control.rs:1022-1027`). |
| `Feedback` | device → binding | `goal_id`, bounded opaque progress | **Not persisted.** It may feed the brain's `observe` view. |
| `Completed` | device → binding → Core | `goal_id`, `device_outcome ∈ {succeeded, failed, cancelled, aborted}`, opaque reason label, `summary_digest`, `trace_ref` | The device's own claim (`NativeSelfReport`). It never verifies anything. |
| `Verdict` | witness → Core | §B.4 | Independent class required. |
| `Cancel` | Core → watchdog | `lease_id`, `goal_id`, reason | Brain-initiated cancel. The lease stays. Reply: `acknowledged`, `not_running` or `unacknowledged`. |
| `Revoke` | Core → watchdog | `lease_id`, next `epochs` | Cancel and latch. Reply: `Revoked{latched, cancel: acknowledged \| no_goal \| unacknowledged}`. |
| `Query` | Core → binding | — | Read-only view for the brain's `observe`. Never evidence. Replaces today's `observe()` sampling. |

**Outcome vocabulary, and who decides each:**

| Question | Values | Decided by | Record |
|---|---|---|---|
| May this goal run? | allowed / refused(reason) | Core admission | decision row (today `physical_decisions`, `store_control.rs:64-74`) |
| Did the device take it? | accepted / refused / **unknown** | The binding reports the device's answer. Core records it exactly; missing, mismatched or late answers become unknown. | goal row disposition plus write callback if late (`store_control.rs:506-579`) |
| What does the device say happened? | succeeded / failed / cancelled / aborted / **unknown** (no event before budget plus grace) | device (self-report) | one completion evidence row |
| Did it physically happen? | verified / contradicted / partial / **unknown** | witness verdict admitted by Core | one consequence row |
| Is the task done? | accepted / rejected / cancelled / pending | Core, automatically or by review | acceptance row (`store_evidence.rs:521-585`) |
| Did the cancel take? | acknowledged / not_running / **unacknowledged** | watchdog | goal row; unacknowledged ⇒ consequence unknown, session quarantined |

**Idempotency and correlation.**

- `goal_id` is Core-minted at admission and never reused (today's `ActionId`, `control.rs:406-408`).
- `op_id` is minted once per dispatch (today's operation id, `control.rs:1028`).
- `lease_id` and `epochs` come from the session reservation (`control.rs:457-475`).
- The device's native handle is recorded only as an opaque digest.
- The watchdog answers a duplicate `(goal_id, op_id)` with its stored answer. The same `goal_id` with a different digest is refused. Core never retries (`docs/device-binding-protocol.md:90`).
- Cross-Host correlation stays `physical-control-v2` semantic ids with replay claims (`remote.rs:246-325`).

### B.4 Evidence model

Persisted per goal: a verdict, a digest and a trace pointer. There is no per-tick series.

| Row | Content | Written when |
|---|---|---|
| decision | proposer, kind, params digest, allowed or refused, reason | every proposal |
| goal (action) | admission audit, then disposition: accepted, refused or unknown | admission, then dispatch return |
| write callback (optional) | exact late or after-close answer (today's `physical_write_callbacks`) | only if late or after close |
| completion | device outcome, reason label, `summary_digest`, `trace_ref` | completion event, or synthesized `unknown` at budget plus grace |
| verdict / consequence | witness class, result, reason, `evidence_digest`, `trace_ref`, ≤ N inline summary samples (`receipt_tick`, `sample_digest`) | once per goal; once more for handover after a cancel |

**Approximate rows.**

- Per task: about 8 fixed rows (attempt, review, session, budget, acceptance, one reservation per domain, plus Root closure updates), plus about 4 per goal (+1 late callback, +1 handover verdict when cancelled).
- A 5-goal task is about 30 rows, **independent of goal duration**.
- Today's 200 ms tick writes about 5 observation rows and up to 5 consequence revisions per second. That is 18,000 of each per hour, with consequence bytes growing quadratically (`docs/physical.md:257-265`).

**Witness independence** is kept by three rules:

1. **Separate component, fixed registry.** The witness is registered at Core start (as today, `core.rs:160-187`). The required class can never be `NativeSelfReport` (`contracts.rs:348-362`). The device's `Completed` event is stored as a self-report and never verifies.
2. **Core checks the verdict, not the physics.**
   - Lineage: goal id, lease, incarnations.
   - Class against the registry.
   - That the verdict was received (executor monotonic ticks) after the completion event, and within `verdict_within_us`.
   - That the inline summary digests to `evidence_digest`.
   - Dwell and continuity are the witness's judgement, attested by the inline summary. Core replays only order and receipt freshness on the executor clock (af69546 rule, `store_control.rs:136-138`).
3. **Content-addressed trace.** The full trace lives outside the ledger (binding or witness storage) under a content address. An auditor can fetch and re-hash it; Core does not need it to decide.

This is a deliberate weakening of today's Core-side recomputation over every stored observation (`evidence.rs:539-596`). See open decision 4.

### B.5 Qualification: declared safety mechanisms

Pastey never claims safety. A binding **declares** its mechanisms as a qualification input. Core checks presence, coverage and digests only. Review shows them as "declared by the binding; not verified by Pastey".

```text
SafetyDeclarationV1 {
  version: 1,
  mechanisms: [SafetyMechanismV1] (1..=16, sorted by id, unique),
  watchdog:   LeaseWatchdogDeclarationV1,
}
SafetyMechanismV1 {
  id:            SemanticIdV1,        // binding-defined, opaque
  layer:         device | binding | external,
  independent_of_pastey: bool,        // keeps working with Pastey, the binding process and the executor Host all dead
  covers:        [HazardClassV1],     // closed Core enum, non-empty, sorted
  config_digest: DigestV1,            // digest of the binding-held configuration (Core never reads it)
  evidence:      { kind: none | self_test | vendor_document | third_party, digest?: DigestV1 },
}
HazardClassV1 = lease_loss | host_loss | link_loss | watchdog_loss | overspeed
              | collision | out_of_region | controller_fault | emergency_stop
LeaseWatchdogDeclarationV1 {
  implementation_fingerprint: ImplementationFingerprintV1,   // reused (descriptor.rs:67)
  min_lease_us, max_lease_us, max_goal_us,
  cancel_acknowledged: bool, cancel_timeout_us,
  clock: monotonic,                                          // required value
}
```

**Refusal rules.** All are fail-closed and checked at qualify, every Review, Start and lease install:

1. No declaration, or an empty `mechanisms` list ⇒ **start refused**.
2. No `watchdog` declaration, or a lease or goal budget outside its min/max ⇒ refused.
3. `watchdog_loss` and `link_loss` must each be covered by at least one mechanism with `layer = device` and `independent_of_pastey = true`; otherwise refused. The watchdog covers `lease_loss`/`host_loss` by construction. Open decision 5.
4. Envelopes that depend on a declared device guarantee need that coverage. An effort cap needs `overspeed`. A `path_contained` claim needs `out_of_region`. Otherwise `effort` and `path` show as "not enforced by the device" and `envelope_claim` must be `target_only`.
5. The declaration digest joins the qualification digest and so the scope digest. Narrowing cannot change it, and any change requires requalification (as fingerprints today, `contracts.rs:287-317`).
6. A self-described binding stays simulation-only (`binding.rs:483-487`, `binding.rs:902-908`). Simulated mechanisms never qualify hardware.

### B.6 `EnvironmentBinding` diff (conceptual)

Today's trait is at `control.rs:380-402`.

| Old method | New | Change |
|---|---|---|
| `describe(host)` | `describe(host)` | **kept**, and returns `SafetyDeclarationV1` and goal kinds |
| `validate_scope(fields)` | `validate_scope(fields)` | **kept** (reject-only, never cached) |
| `status()` | `status()` | **kept**, called at dispatch and on binding events only, not per sample |
| `witnesses()` | `witnesses()` | **kept**. The witness API changes: `completion(goal summary)` and `handover(post-cancel snapshot)`; `effect_bound` over the summary |
| `evaluate_start(predicate)` | `evaluate_start(predicate)` | **kept** |
| `install_session(view)` | `install_lease(view)` | **changed**: lease semantics (§C); evidence class unchanged |
| `apply(view)` | `dispatch_goal(view)` | **changed**: kind, params, digest and relative budget instead of an option name; one call per goal |
| `fence(view)` | `revoke_lease(view)` | **changed**: returns the cancel outcome (`acknowledged` / `no_goal` / `unacknowledged`) |
| `observe()` | — | **removed** as a Core-polled sample (`BindingSampleV1`, `control.rs:217-222`) |
| — | `query_view()` | **new**: read-only brain view for `observe`; never evidence |
| — | `cancel_goal(view)` | **new**: brain or Core cancel without dropping the lease |
| — | `renew_lease(view)` | **new**: coarse renewal (§C.4) |
| — | `next_event()` | **new**: completion and binding-fault events pushed to Core; feedback stays binding-local |

Views lose `LaneValidityV1`'s `continuing` horizon (`control.rs:155-172`). A goal view carries only the relative goal budget and lease remaining. Validity across machines is never shared by reference.

### B.7 Adapter sketches (outside Core)

#### B.7.1 ROS 2: rosbridge client on the Mac Host plus a robot-side node

- **Mac Host (executor).** A Rust binding implements the trait (§B.6) as a **rosbridge** websocket client. It talks only to the robot-side Pastey node's interfaces, never to Nav2 directly, so every goal passes the watchdog.
  - `describe` reports an implementation fingerprint covering the node's version and the Nav2/ros2_control configuration digests.
  - `query_view` reads a bounded pose and status summary.
- **Robot-side node `pastey_lease_watchdog`** (rclpy or rclcpp). It holds the lease (§C) and exposes:
  - `/pastey/lease/{install,renew,revoke}` services;
  - a `/pastey/goal` interface that forwards a goal to Nav2's `navigate_to_pose` action (`nav2_msgs/action/NavigateToPose`);
  - feedback summarized from `distance_remaining`, `estimated_time_remaining` and `navigation_time` (not persisted);
  - the result as action status (`SUCCEEDED`/`ABORTED`/`CANCELED`) plus Nav2 `error_code`/`error_msg`. *Unverified per distro*: the result fields differ across Nav2 releases.
  - On lease loss, revoke or rosbridge disconnect it calls the action cancel and latches.
- **Device fail-safe (declared, §B.5).**
  - A ros2_control controller command timeout: zero velocity if commands stop. *Unverified* exact parameter, for example a `diff_drive_controller` command timeout.
  - Nav2 velocity smoother limits (`overspeed`).
  - Nav2 Collision Monitor (`collision`).
  - A velocity gate fed by the watchdog's heartbeat, such as a mux lock (`watchdog_loss`). *Unverified* which gate the chosen stack provides.
- **Witness.** Independent: an external camera or AprilTag localizer, or motion capture (`IndependentMeasured`). In simulation, Gazebo ground truth (`SimulationOracle`). AMCL or odometry is `NativeSelfReport` and can never verify.
- *Unverified*: rosbridge support for ROS 2 actions in the target distro. Fallback: the node exposes a service (`goal` → accepted/refused) plus a completion topic, which needs no action support in rosbridge.

#### B.7.2 MicroDuck firmware: command style

- **Protocol.** The binding talks serial or Wi-Fi frames to the firmware. Each one maps one-to-one to §B.3:
  - `LEASE{id, epoch, ttl_ms}` → `LEASED`/`NAK`
  - `RENEW{id, seq, ttl_ms}`
  - `CMD{id, epoch, goal, kind, params, budget_ms}` → `ACK`/`NAK`
  - `DONE{goal, outcome, reason}`
  - `REVOKE{id, epoch}` → `REVOKED{cancel}`
- **Watchdog.** It is in the firmware itself: the lease deadline is a monotonic millisecond counter checked in the main loop; expiry or link loss runs the firmware's declared safe action (stop and sit); the latch holds until reboot (R1, R21).
- **Execution.** Gait and policy are entirely device-owned. Pastey sees `kind` and digests only.
- **Witness.** External: a camera or a second device. The firmware's own `DONE` is a self-report.
- **Size.** About 150 lines of C in the firmware plus about 400 lines for the Rust binding (estimate).

#### B.7.3 Vendor SDK (Unitree-like)

- Vendor SDKs typically expose high-level velocity or posture calls with internal timeouts rather than goals (*unverified* for any specific SDK version).
- The binding therefore runs a small shim on the robot's companion computer. The shim turns a goal into device-side execution with its own local pursuit loop. That loop is binding-owned HOW, not Pastey's.
- The shim hosts the same watchdog state machine. It declares the SDK's internal command timeout (`link_loss`/`watchdog_loss`) and the vendor's obstacle avoidance (`collision`) as mechanisms, with the vendor document digest as evidence.

#### B.7.4 The contract needs no ROS concepts

| Contract (§B.3) | ROS 2 adapter | Firmware adapter | Vendor-SDK adapter |
|---|---|---|---|
| lease install / renew / revoke | node services | `LEASE` / `RENEW` / `REVOKE` frames | shim RPC |
| dispatch(goal) | Nav2 navigate goal via node | `CMD` | shim goal, local pursuit loop |
| feedback | action feedback | none or `PROGRESS` | shim telemetry |
| completion event | action result status plus error code | `DONE` | shim result |
| cancel | action cancel | `REVOKE` / `STOP` | SDK stop call |
| device fail-safe | ros2_control timeout, smoother, Collision Monitor | firmware loop timeout | SDK internal timeout |

Core sees only the left column, as opaque kinds, digests, bounds and classes. `check:core-agnostic` is unaffected because adapters live outside `src-tauri/src/physical` Core files.

---

## Part C: Thin binding-side lease watchdog

### C.1 Placement

The watchdog is a separate component **next to the robot**:

- ROS: the robot-side node.
- Firmware: inside the firmware.
- Vendor SDK: inside the companion shim.

It is not part of the executor Host and does not depend on Core, the Bridge or the binding process staying alive. The binding in the executor Host is its only lease holder (R7).

### C.2 What it holds

The state is RAM only. A restart is a new incarnation (R1).

```text
incarnation          fresh per process start
high_water_epochs    per conflict domain (RAM); Core's durable epochs (store.rs:1227-1272) stay authoritative
lease                { lease_id, epochs, holder_connection, deadline_mono, max_goal_us, renew_seq } | none
goal                 { goal_id, op_id, params_digest, device_handle, goal_deadline_mono, state } | none
latch                none | revoked | expired | link_lost | faulted
```

### C.3 What it does

It runs a single-threaded state machine. Revoke, dispatch, renew and the local timers are serialized (R17, R18 at goal level).

| Event | Action |
|---|---|
| process start | Cancel any device goal it can find ("cancel-all-on-start"). Latch `none` with no lease: refuse goals. |
| `LeaseInstall` (epoch > high-water, duration within declared min/max) | Set `deadline_mono = now_mono + duration − margin`; reply `LeaseInstalled`. A stale or foreign install is refused (R4, R8). An exact duplicate is idempotent with no extension (R5). A newer one cancels the old goal first (R6). |
| `Renew(seq > last)` while not latched | `deadline_mono = now_mono + extend_us − margin`, capped by the install's Root-remaining cap. Never revives a latched lease. |
| `Dispatch` | Refuse unless the lease is valid, not latched, no goal is running (or the declared preemption allows it), and `budget ≤ min(max_goal_us, lease remaining)`. Otherwise forward to the device, set `goal_deadline_mono`, and reply with the device's accept or refuse. |
| goal deadline reached | Cancel the goal (budget exhausted). The lease stays and the event is reported. |
| lease deadline reached | Cancel the goal; latch `expired`; refuse everything until a new `LeaseInstall` with a higher epoch. |
| `Revoke` | Cancel the goal; latch `revoked`; reply with the cancel outcome. A duplicate is idempotent and a stale one is inert (R16, R19). |
| holder link lost | Cancel the goal; latch `link_lost` (R20). A reconnect needs a new lease. |
| cancel fails or times out (`cancel_timeout_us`) | Retry the cancel; it is idempotent and device-local. Then latch `faulted` until process restart (R21) and report `unacknowledged`. |
| device completion | Clear the goal and forward `Completed`. The lease stays. |

**It never resumes.** A latched lease is never un-latched by a renew, dispatch or reconnect. Only a new grant can produce a new lease (I4).

### C.4 How it learns of expiry without per-200 ms Core supervision

- **Local monotonic deadline (preferred).**
  - Every lease and goal duration is sent **relative**, and turned into a deadline on the watchdog's own monotonic clock at receipt.
  - Wall clocks are never compared across machines; ticks mean something only inside the process that wrote them (`af69546`, `docs/physical.md:58`).
  - The margin covers one transport delay.
- **Coarse renewal pushed by Core.**
  - Core runs one timer per stream at `renew_interval_us`. Proposed defaults: lease L = 6 s, renewal R = 2 s (open decision 2).
  - A renewal is sent only if, at that instant: the Root validates; the route is `Connected`; the brain is within its idle lease; and the Root, approval and qualification have not expired.
  - Core reads no device state to renew.
  - Every other Core-side timer is one-shot (`sleep_until`): idle lease, goal budget plus completion grace, Root expiry. Each is re-armed on the relevant event.
- **Revoke pushed by Core immediately**, never at the next tick, on any of:
  - Cancel or revoke;
  - Burn or purge (today's `invalidate_physical_bridge`, which must also push the revoke, A.5 #1);
  - the route being permanently gone (below);
  - a committed MCP session closing (`decision_tools.rs:665-692`);
  - idle lease expiry;
  - an admitted contradiction;
  - a policy or qualification change (`core.rs:246-284`);
  - Core shutdown (`core.rs:777-782`).
- **If the executor Host or binding process dies**, nothing renews. The watchdog cancels within at most L after the last renewal, or at once when it sees its holder link close.
- **If the brain crashes**, Core learns from the MCP close (immediate) or the idle lease (one-shot timer), then revokes.

**When Bridge loss counts as "permanently gone"** (brain Host ⇄ executor Host):

- A reconnect always mints a new peer session, and the old route is marked stale and never rebinds (`docs/layers/layer-4-bridge.md:9`, `:15`).
- A Root is bound to the exact `HostSessionBinding` (`core.rs:64-73`).
- So a stream can never survive a reconnect. "Permanently gone" for a Root is:
  1. Burn, purge or explicit departure (`host_runtime.rs:272-282`; `layer-4-bridge.md:11`);
  2. the route replaced (a different session pair);
  3. liveness `disconnected`, `left`, `stale` or `expired`;
  4. liveness `reconnecting` for longer than the in-flight grace `G`.

**Grace rule.**

- New dispatches are refused as soon as liveness is not `Connected`. This is today's behavior (`bridge_lifecycle.rs:98-107`).
- An in-flight goal is revoked once the route is permanently gone. `G` defaults to 0, matching today's semantics (any non-`Connected` state fails the next route check).
- A larger `G` only lets the current goal finish while no brain can observe or cancel it. The executor-local revoke still works during that time. Open decision 3.
- Detection must be event-driven: the lifecycle probe already runs every 2 s and marks `reconnecting` (`bridge_lifecycle.rs:289-322`). That transition must invalidate the Physical peer the way `purge_room` does.

The watchdog's own link to the executor Host needs no grace rule: a holder link close ⇒ cancel (R20 parity).

### C.5 What it explicitly does not do

- No collision, joint-limit, velocity or controller logic.
- No path monitoring or geofencing.
- No retries of goals (only of the cancel).
- No evidence production beyond forwarding device events.
- No persistence of authority across restarts.
- No interpretation of goal parameters beyond digest equality.

The device stack's declared mechanisms (§B.5) do all of that.

### C.6 Failure modes of the watchdog itself

| Failure | Detection | Body | How it surfaces (never as success) |
|---|---|---|---|
| Watchdog process dies | Binding sees its link close or renewals fail | The device mechanism covering `watchdog_loss` must stop it; otherwise the goal runs to the device's own end | Core: lease lost ⇒ Root closed, cancel outcome **unknown**, consequence **unknown**, session quarantined. A restarted watchdog has a new incarnation, so old results are rejected (R1, R3). |
| Cancel call fails | Device API error | Retried, then `faulted` latch | `Revoked{cancel: unacknowledged}` ⇒ consequence **unknown**; quarantine until a handover verdict verifies a safe state (today's rule: a fence ack alone keeps quarantine, `docs/physical.md:73`, stage5 `fence_ack_alone_keeps_unknown_quarantine`) |
| Cancel acknowledged but the body keeps moving | The witness's post-cancel verdict is not "at rest" | Device's responsibility | Handover never verifies, so the domains stay quarantined |
| Revoke push undelivered | Binding send error or no reply | Watchdog lease expiry or link-loss rule | Core records the cancel outcome as **unknown** |
| Renewal starved (Wi-Fi blip, GC pause) | Lease expires | Cancel and latch | Spurious `expired`: the stream ends **uncertain**; no resume (cost of a small L) |
| Duplicate or late frames | Epoch, `goal_id` and `op_id` checks | none | inert (R19, R22) |
| Device emits no completion | Core one-shot timer at budget plus grace | Watchdog has already cancelled at the budget | Completion **unknown** |

### C.7 Mapping from existing rules, and what is dropped

| Existing | Becomes |
|---|---|
| R13 self-stop at action and lease deadlines | goal-budget cancel plus lease-deadline cancel |
| Fence serialization R16–R18 (`sim.rs:510-552`) | revoke serialized with dispatch in the state machine |
| Latch R21 (`sim.rs:634-636`) | `faulted` latch; also `revoked`, `expired` and `link_lost` latches |
| R14 never resume, R20 connection loss | latch semantics |
| Loss profile (`contracts.rs:454-457`) | the watchdog declaration plus device mechanisms (§B.5) |
| Core `stream_tick` idle and route checks (`decision_tools.rs:451-464`) | Core one-shot timers plus event-driven revoke |

Dropped:

- R11 sequenced refresh;
- R13's refresh-loss interval;
- R15 control-loop rule (device);
- the R17 actuation critical section (device);
- R19a continuous post-fence evidence (a single snapshot remains);
- `LaneValidityV1` cross-device validity handles (`control.rs:155-172`).

### C.8 Size

Estimates; nothing is implemented.

| Item | Lines |
|---|---|
| Watchdog state machine plus protocol (portable core) | 250–350 |
| ROS node wrapper | +150–250 |
| Firmware version (C) | ~150 |
| Core-side lease manager (renew timer, revoke push, one-shot timers) | 200–300 Rust |

It replaces:

- the supervisor (~130, `decision_tools.rs:64-110`, `:434-521`);
- observation ingestion (~210, row 11);
- per-tick evaluation and effect bound (~150, `decision_tools.rs:522-614`);
- in the reference runtime, the R-rule runtime of `sim.rs` (~240, `sim.rs:367-602`).

---

## Part D: Feasibility spike plan (plan only)

### D.1 Shape

- Add `InvocationModeV1::Goal` next to `DecisionStream`. Leave DecisionStream's semantics, freshness, scheduler, single-flight and Refused/Unknown behavior untouched (standing constraints).
- Write a simulated goal body: the flat from `bindings/flat.rs` with a "go to named target" kind and its own internal pursuit loop.
- Write a watchdog sim: the C.3 state machine in Rust, test-only.
- Drive it through the existing remote protocol and tool dispatcher, under one Core with no tick supervisor running.

### D.2 Success criteria as automated tests

All of these are new and do not exist yet. Each test also asserts that no `stream_tick` ran: a test-only supervisor counter equals 0.

| Criterion | Test name | Asserts |
|---|---|---|
| 1. Brains replaceable | `physical_goal_demo_1_brains_are_replaceable_under_one_approval` | A rule brain and a mock-LLM brain each reach the bedroom with ≥2 goals under an identical approval digest. The tool list is the approved kinds plus `observe`, `remaining_budget` and `cancel`. One approval per run. The witness is `Verified` and the sim agrees. |
| 2. Bad brain stays inside the envelope | `physical_goal_demo_2_a_bad_brain_cannot_leave_the_envelope` | Refused and recorded: a target outside the waypoint set or box, an effort above the cap, a budget above `max_goal_us`, an unapproved or undeclared kind, a zero budget, a flood faster than `min_goal_interval_us`. Cumulative goals and time never exceed the approval, and `remaining_budget` is exact. Sim truth: only approved targets were pursued, every goal ended by its budget, and no refused goal reached the body. **Not asserted**: path containment, unless the sim declares `out_of_region`. |
| 3. Revoke, disconnect, crash and host loss stop the body and never resume | `physical_goal_demo_3_revoke_disconnect_crash_and_host_loss_cancel_and_never_resume` | For each of revoke, Bridge permanently gone (event, not poll), brain crash (MCP close, and a silent brain via idle lease) and executor Host loss (Core dropped, no renewals): the watchdog cancels within the stated bound (immediate, or ≤ L for host loss), the sim body stops, the consequence is `Uncertain`, the watchdog is latched, any further goal is refused until a new approval, and recovery dispatches nothing. |
| 3b | `physical_goal_watchdog_unacknowledged_cancel_is_unknown_and_quarantined` | A cancel failure gives a `faulted` latch, consequence `Unknown`, and the domain stays quarantined. |
| 4. Records apart; witness judges arrival | `physical_goal_demo_4_proposer_admission_dispatch_completion_and_witness_are_recorded_apart` | Per goal there is a decision row, a goal row, a completion row and a verdict row. The record count equals allowed + refused + 1. A device `succeeded` without a verdict is not Verified. A device `succeeded` with a contradicting verdict is `Contradicted`. |
| 4b | `physical_goal_ledger_rows_do_not_grow_with_goal_duration` | A 10 s goal and a 60 s goal produce the same number of rows. |
| 5. Bodies replaceable | `physical_goal_demo_5_bodies_are_replaceable_under_the_same_core` | A dispenser `fill_to` goal body and a firmware-style command body run under the same Core with no Core change. Refusals, budgets and revocation behave as in 2 and 3. `check:core-agnostic` = 0. |
| Qualification | `physical_goal_start_is_refused_without_declared_safety_mechanisms` | Missing declaration, missing watchdog, or missing `watchdog_loss`/`link_loss` coverage each refuse qualification and start. |

### D.3 Core parts bypassed, stubbed or reused

- **Bypassed** (the goal mode never calls them):
  - `supervise_stream` and `stream_tick`;
  - `record_control_observation`, `issue_proposal_challenge`, `continuing_deadline` and `BindingSampleV1.control`;
  - per-tick `check_effect_bound` and `evaluate_stream`;
  - `LaneValidityV1.continuing`.
- **Stubbed.** `ReviewScopeFieldsV1.validate` requires freshness fields (`contracts.rs:401-479`). A goal scope uses its own fields rather than reinterpreting them, so DecisionStream's freshness semantics stay unchanged.
- **Reused unchanged:**
  - review, approval, Root, basis and narrowing (`core.rs`);
  - reservation and epochs;
  - budgets;
  - `physical_decisions`;
  - the action row as the goal row;
  - `prepare_write`/`finish_write`/`physical_write_callbacks`;
  - `close_root`, quarantine and `fence_request`;
  - acceptance;
  - the witness registry and class rules;
  - `physical-control-v2`, MCP transport and grants;
  - Bridge flow control.
- **New:**
  - `BoundSetV1::admits`;
  - goal scope and admission;
  - the lease manager (renew timer, revoke push, one-shot timers);
  - a liveness-transition hook from the Bridge lifecycle to `invalidate_physical_peer`;
  - `next_event` ingestion;
  - the verdict-with-summary format.

Tick-specific parts found while planning this (fed back into A.2): items 1–12 there. The handover trace (A.1 row 23) is also partly tick-shaped: it requires a continuous run of post-fence samples (`evidence.rs:848-890`).

### D.4 Risks, unknowns, size

**Risks and unknowns:**

1. Acceptability of the `target_only` trade-off for region envelopes (open decision 1).
2. Spurious lease expiry versus stop latency when choosing L and R (open decision 2).
3. Adding `Goal` rows to the existing tables probably needs a ledger format bump (6 → 7, `store.rs:116`) and a development reset (`docs/development.md`). *Unverified* whether additive stages suffice.
4. MCP tool definitions with parameters, and whether agents produce valid bounded parameters reliably.
5. A truly independent witness on hardware, which no code has yet (A.5 #5).
6. The ROS unknowns listed in §B.7.1 (*unverified*).
7. Whether to keep both modes long-term, which doubles the test surface.
8. The Core lock still serializes everything. The goal mode removes the per-tick acquisitions, but a full ledger audit still runs under the Core lock (`docs/physical.md:252`).

**Size (rough):**

| Part | Lines | Days |
|---|---|---|
| Core goal mode | 1,200–1,600, of which ~600 tests | 6–9 |
| Simulated goal body plus watchdog sim | ~500 | 2–3 |
| Smallest ROS step (D.6) | node ~400 (Python) plus Rust rosbridge binding ~600 | 4–6, dominated by environment setup |

Total: 2.5–3.5 weeks.

### D.5 Existing tests: obsolete versus still valid

269 tests in `src-tauri/src/physical` at HEAD, counted by `#[test]`/`#[tokio::test]` per file. "Obsolete" means it tests a tick-specific mechanism. While DecisionStream stays, all of them keep passing as regression tests for that mode. They become deletable only when the mode is retired.

| File (count) | Obsolete at goal granularity (rewrite or delete) | Still valid |
|---|---|---|
| `stream_tests.rs` (31) | `ticks_run_on_deadlines_so_tick_work_does_not_widen_the_sample_gap`, `an_overrun_runs_the_next_tick_at_once_without_a_catch_up_burst`, `tick_work_beyond_the_gap_limit_still_fails_closed`, `the_supervisor_stops_once_the_stream_has_ended`, `appending_evidence_costs_the_same_with_a_long_history`, `a_witnessed_effect_bound_violation_ends_the_stream_and_is_recorded` (rewrite as end-of-goal); rewrite as events: `the_timer_ends_an_idle_stream_like_a_crashed_brain`, `the_timer_ends_a_stream_whose_budget_is_spent`, `a_lost_binding_ends_the_stream_before_any_further_write`, `each_decision_is_admitted_recorded_rate_limited_and_replaces_the_last` | tool surface, refusal, budget, tool-session lifecycle, start predicate, observation filtering, remote tool routing, intent-only bound; the 9 `append_path` ledger-integrity tests while the evidence table exists |
| `stage4_tests.rs` (63) | `observation_source_age_gap_order_and_replay_are_checked`, `fresh_observations_maintain_validity_only_inside_fixed_horizon`, `observation_expiry_ends_the_action_without_revival`, `a_replaced_challenge_voids_the_old_one_and_freshness_boundary_is_closed`, `wrong_challenge_observation_sequence_action_and_payload_fail_closed` (challenge part), `an_expired_deadline_and_lapsed_observation_freshness_are_told_apart` (freshness half) | reservation and epoch races, install, dispatch intent, all write-callback and late-result tests, revoke races, restart, ledger tampering (~56) |
| `stage5_tests.rs` (35) | series-shaped: `inclusive_bounds_dwell_and_continuity_are_required`, `missing_source_sequence_and_initial_observation_interval_fail_closed`, `late_stale_receipt_does_not_retroactively_fill_a_dwell`, `motion_after_verified_dwell_requires_a_new_held_dwell`, `a_late_proven_violation_refines_unknown_without_erasing_the_gap`, `duplicate_old_samples_and_dispositions_do_not_advance_freshness`, `reordered_trustworthy_contradiction_adds_history_without_renewing_freshness`, `a_new_executing_disposition_cannot_reuse_an_old_terminal_hold_anchor`, `partial_then_verified_and_late_contradiction_append_history` | acceptance, cancel/accept races, fence-ack quarantine, handover release rules, restart, qualification withdrawal, wrong Host, clock regression, simulation/hardware boundary (~26) |
| `witness_tests.rs` (8) | `core_recomputes_verdict_window_and_evidence_instead_of_trusting_them`, `a_stale_trace_is_unknown_even_when_the_witness_claims_verified` (rewrite to summary level) | class rules, startup verdict check (6) |
| `bounded_validation_tests.rs` (8) | `an_admitted_action_keeps_its_lifetime_on_a_long_history` | 7 (the ledger stays) |
| `physical_demo_tests.rs` (8) | the 5 criteria tests, `physical_demo_verified_acceptance_then_fence_leaves_no_action_executing` and `a_policy_change_closes_only_its_own_environment`: replaced by goal versions | `the_development_switch_offers_a_reference_body_and_fails_closed` |
| `mcp_tests.rs` (11) | tool-schema parts of `an_mcp_client_walks_the_body_to_the_bedroom_through_the_brain_hosts_relay` and `the_relay_forwards_every_call_and_the_executor_judges_it` | connection, grant and commit lifecycle (9) |
| `tests.rs` (48), `stage3_tests.rs` (29), `stage7_tests.rs` (15), `capability_tests.rs` (6), `descriptor_tests.rs` (5), `ledger_format_tests.rs` (2) | DecisionStream-shape tests become mode-specific: `decision_stream_scopes_stay_inside_the_declared_capability`, `decision_stream_narrowing_only_drops_options_and_slows_the_rate`, `proposal_observation_freshness_and_execution_budget_are_independent` | identity, enrollment, scope hashing, review/approval/Root, remote protocol, capability genericity, descriptors, ledger format (~102) |

In total, about 40 of 269 are obsolete or need rewriting at goal granularity; about 229 stay valid.

### D.6 Smallest ROS adapter step (adapter material)

1. Nav2 in simulation (a TurtleBot3-style Gazebo bringup, *unverified* exact package names for the chosen distro) on a Linux machine or container. No real robot.
2. Robot-side `pastey_lease_watchdog` in rclpy implementing §C.3. It wraps `navigate_to_pose` and declares the controller command timeout, velocity smoother and Collision Monitor in `SafetyDeclarationV1`.
3. A Mac Host binding as a rosbridge client (port 9090 by default, *unverified* for the deployment) speaking only to `/pastey/*` interfaces.
4. Witness: simulator ground-truth pose (`SimulationOracle`).
5. Proven by this step:
   - an out-of-envelope goal is refused by Core and never reaches Nav2;
   - revoke cancels the Nav2 goal;
   - killing the Mac Host cancels it within L;
   - a disconnected rosbridge cancels it immediately;
   - a cancelled goal never resumes.

### D.7 What is reusable from committed fixes

| Commit | Reusable |
|---|---|
| `1c55c0e` | The split between "may it still run" and "what did the binding answer" (`control.rs:1085-1094`), exact answers recorded regardless of authority, and history-only callback rows. This maps directly onto `dispatch_goal` answers that arrive after a revoke. |
| `7d1a047` | Judging the goal budget at the tick the dispatch returned, so Core lock waits never expire a timely answer. Lapsed versus revoked reasons (`control.rs:101-118`, `control.rs:959-1009`). |
| `af69546` | The single monotonic rule `deadline_reached(tick, deadline) = tick >= deadline` (`store_control.rs:136-138`); ticks meaningful only inside the process that wrote them; wall time informational. The same principle underlies the watchdog's relative durations (§C.4). |
| `3376e1e` | Bounded Root-group validation and declared write kinds (`store.rs:359-655`) stay useful while the ledger exists. Its motivating cost, long per-tick histories, largely disappears, so it becomes a safety margin rather than a necessity. |
| `c8556b0`, `a193eda` | Read-only route checks and separate Physical flow control. Both are kept as-is. |

---

## Appendix: Open decisions

Ranked; each with a recommendation and the cost of being wrong.

1. **Envelope claim for regions.**
   - Recommendation: allow region envelopes only with a declared device `out_of_region` mechanism. Otherwise allow waypoint sets labelled `target_only`.
   - If wrong one way, Review overclaims containment. If wrong the other way, ROS adoption is blocked.
2. **Lease parameters and renewal.**
   - Recommendation: a local monotonic deadline, L = 6 s, R = 2 s, with renewals gated on Core authority, plus event-driven revoke.
   - A large L means the body runs up to L after the executor Host dies. A small L gives spurious cancels on Wi-Fi or GC hiccups.
3. **Bridge-loss in-flight grace `G`.**
   - Recommendation: `G = 0`, matching today, with an event-driven liveness hook.
   - `G > 0` lets a goal run with no brain able to see it. `G = 0` aborts goals on any 2 s probe failure.
4. **Evidence model.**
   - Recommendation: a verdict with a bounded inline summary (≤32 samples) plus a content-addressed external trace, accepting the ledger format bump.
   - A pointer-only verdict makes audits depend on external storage. Keeping the full series keeps the growth problem.
5. **Watchdog-death coverage required before start** (§B.5 rule 3).
   - Recommendation: require it.
   - Not requiring it leaves a crashed watchdog with the device driving until goal end. Requiring it adds a velocity-gate component to the first ROS step.

Further decisions not in the top five:

- whether `Goal` replaces `DecisionStream` (recommendation: coexist until the spike passes, then retire DecisionStream);
- the fate of NativeFence (recommendation: delete the placeholder in the goal mode);
- whether re-grant stays gated on a handover verdict (A.5 #7).
