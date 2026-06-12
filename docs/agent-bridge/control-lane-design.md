# Control Lane Design

## 1. Status and Scope

This is a design and feasibility document. It does not change runtime behavior.

The proposed control lane is a semantically separate path for Agent Bridge and
other bounded room-control events. It is not a user text path, file-transfer
path, MicroFlowGroup path, execution path, or permission grant.

The control lane:

- does not authorize execution;
- does not imply peer consent;
- does not add a peer executor;
- does not change binary-v1 or binary-v2 behavior;
- does not require binary-v2 initially, although the design may inform a later
  binary-v2 control-frame or substream design.

The proposed capacity model uses eight logical window-equivalents as a
scheduler/resource-allocation concept. The binary-v1 default runtime window of
`8` came first as the tested lower-level capacity basis. The frontend planner's
`globalWindowBudget = 8` was then set to represent that same outgoing runtime
capacity at the scheduler level. They are two layers of one logical outgoing
capacity model, not independent or additive pools.

A future shared inbound/outbound control lane therefore reserves one logical
window-equivalent from the planner-level representation of that existing
capacity. It does not create a ninth lane or treat binary-v1 as a separate
hidden pool. The lane still requires explicit control queue/accounting and a
separate room-control transport path because current inbound room items are
handled outside the outgoing file planner.

## 2. Problem

The current text send path is:

```text
RoomPage.tsx::handleSendText
  -> src/lib/tauri.ts::sendTextToRoom
  -> commands::send_text_to_room
  -> storage::create_outgoing_text_item
  -> transfer::send_room_item
  -> POST /rooms/:room_id/items
  -> transfer::receive_item_handler
  -> storage::persist_incoming_item
```

This path creates an encrypted, persisted `PayloadType::Text` room item. The
receiver persists it as an incoming room item, and the UI renders it with
`item_kind = "text"`.

Using this path for capability preview or room-control messages would:

- reinterpret ordinary encrypted user text as control traffic;
- persist control events as user content;
- blur normal room-history and control-state UI semantics;
- create a risk of hidden control payloads inside text messages;
- touch existing room-item transfer and persistence semantics;
- conflict with the Agent Bridge safety boundary.

The current AI Slot Phase E1 implementation correctly blocks actual room
transport. It builds and validates capability-preview envelopes locally, then
simulates inbound acknowledge or deny state without calling `sendTextToRoom`.

## 3. Proposed Model

The proposed logical capacity split is:

```text
total logical runtime capacity: 8 window-equivalents

when control backlog exists:
  control lane: at most 1
  data lanes: at most 7

when control backlog is empty:
  control lane: 0 reserved
  data lanes: may use all 8
```

This is a **hard semantic separation** with a **soft capacity reservation**:

- control events use a typed room-control path, never user text or file paths;
- one logical window-equivalent is available to control work only while a
  control backlog exists;
- data work may borrow that capacity when the control backlog is empty;
- inbound and outbound control work share the same single logical lane;
- inbound replies and errors generally receive priority over outbound work;
- every control event is typed, validated, small, and bounded.

The control lane is not an "AI lane." It is a room/control/event lane that can
serve Agent Bridge preview events and future bounded room-level control or
status events.

The scheduler-level `globalWindowBudget` is the correct representation from
which to reserve control capacity. When valid control backlog exists, a future
resource arbiter exposes `7` windows to the data planner and holds at most `1`
for the shared inbound/outbound control queue. When no control backlog exists,
the data planner receives all `8`. This reservation preserves the lower-level
binary-v1 capacity basis; the binary-v1 hot path does not own or schedule the
control lane.

The future implementation must still define control backlog and control-lane
occupancy explicitly. Reducing the data budget to `7` creates the capacity
reservation, but does not by itself create the typed room-control transport or
shared inbound/outbound control queue.

## 4. Control Lane Internal Scheduling

Proposed internal priority:

```text
Priority 1: inbound deny / error / expired / invalid
Priority 2: inbound acknowledge / status
Priority 3: outbound user-confirmed preview or control request
Priority 4: outbound preview or status refresh
```

Inbound closure events should not be starved:

- the sender UI needs prompt state closure;
- deny, error, expired, and invalid events should resolve pending state quickly;
- outbound requests must not flood or indefinitely delay replies;
- fast closure reduces stale pending previews and repeated user actions.

There is no fixed half-outbound and half-inbound split. Both directions share
one logical lane, and the scheduler selects the highest-priority eligible
event. Payload size caps, event-rate limits, queue bounds, expiry, and duplicate
detection are required so either direction cannot monopolize the lane.

The scheduler should also prevent permanent outbound starvation. After urgent
inbound closure events are drained, bounded fairness or aging may allow an
older eligible outbound event to proceed.

## 5. Allowed Initial Event Kinds

The first control-lane phase may allow only preview events:

```text
capability_preview
capability_preview_ack
capability_preview_deny
capability_preview_invalid
capability_preview_expired
```

Future event kinds, not implemented yet:

```text
capability_request
capability_request_ack
capability_request_deny
capability_result
```

Explicitly forbidden:

```text
execute_command
run_code
shell
process_spawn
stdout_stream
file_transfer_over_control_lane
scheduler_mutation
microflowgroup_mutation
```

The allowlist must be closed by default. Unknown event kinds and unexpected
fields must be rejected, not forwarded or interpreted.

## 6. Relationship to Existing Paths

| Concern | Data/Text/File Path | Control Lane |
| --- | --- | --- |
| user text | yes | no |
| file bytes | yes | no |
| MicroFlowGroup | data optimization only | no |
| capability preview | no | yes |
| capability result | no | future bounded only |
| persistence | user-content rules | separate decision |
| scheduler windows | sender-side data planner | one logical control reservation |
| execution authority | no | no |

The current planner contains model-level task-kind names for `text`, `control`,
`agent`, and `command`, plus a `control_text` classification lane. These names
do not have a current queue adapter or dispatch path. They must not be treated
as an implemented control transport or authority boundary.

## 7. Relationship to MicroFlowGroup

Control events never enter MicroFlowGroup.

MicroFlowGroup remains a data-transfer optimization for eligible queued
file-like items. Its fixed and dynamic modes, eligibility rules, grouping
thresholds, serial child dispatch, lifecycle, and diagnostics should not be
changed by the control-lane design.

In a future reservation stage:

1. A resource arbiter determines whether a valid control backlog exists.
2. If it exists, the arbiter exposes at most seven logical windows to the data
   planner; otherwise it exposes all eight.
3. The existing data planner independently allocates those available data
   windows across ordinary file-like tasks and synthetic MicroFlowGroup tasks.
4. A MicroFlowGroup continues to consume exactly one data-planner window and
   sends its children serially through the existing single-file path.

The control lane must not inspect, create, mutate, regroup, launch, cancel, or
otherwise control MicroFlowGroup work.

## 8. Relationship to Binary-v1 / Binary-v2

The initial control-lane design should not require binary-v2.

Binary-v1 file and text transfer semantics must not be reinterpreted. In
particular, capability events must not be tunneled through
`sendTextToRoom`, `send_text_to_room`, `PayloadType::Text`, or
`transfer::send_room_item`.

No safe binary-v1 room-control path was found in the current source search.
The room server has routes for room join, legacy room items, file transfer,
diagnostics, burn, and leave, but no typed capability or generic room-control
event route. This is an implementation gap to preserve, not a reason to tunnel
control data through user content.

A future safe room-control transport could be a bounded typed route protected
by the existing room transport identity/key context, provided its schema,
authentication, replay handling, persistence, rate limits, and UI semantics
are designed explicitly. Binary-v2 may later provide protocol-native control
frames or substreams, but that is not required for this feasibility phase.

## 9. Capacity and Fairness Rules

- The control lane has at most one logical window-equivalent.
- Data may use all eight logical windows when no valid control backlog exists.
- Data should yield to at most seven logical windows while a valid control
  backlog exists.
- The eight-window scheduler budget and binary-v1 default runtime window are
  two layers of the same logical outgoing capacity model, not additive pools.
- The control reservation comes from the scheduler-level representation of
  that capacity; it does not create a ninth lane.
- The control reservation must be released promptly when the control backlog
  becomes empty, invalid, expired, disabled, or disconnected.
- Inbound and outbound control queues share the same logical lane.
- Inbound deny, error, expired, and invalid events have highest priority.
- Control events must be small, bounded, typed, authenticated, and validated
  before they can create backlog demand.
- Queue length, event rate, retry count, and time-to-live must be bounded.
- A future bounded capability result must have strict schema and byte caps.
- Oversized outputs must be rejected from the control lane.
- Large results or artifacts require an explicit user-visible file transfer.
- Raw stdout, stderr, logs, secrets, and streaming output must not use the
  control lane.

A backlog must mean validated, eligible control work. Invalid or unauthenticated
traffic must not be able to hold data at seven windows indefinitely.

The reservation is a logical policy unit, not necessarily one binary-v1 chunk
request. The transport-specific mapping remains an open design decision.

## 10. Feasibility Review from Current Code

### Current Eight-Window and Runtime-Window Model

| File / function | Current role | Future control-lane relevance | Avoid or preserve |
| --- | --- | --- | --- |
| `src/lib/transferPlanner.ts::DEFAULT_TRANSFER_PLANNER_POLICY` | Defines `globalWindowBudget = 8`, `minRequestedWindow = 1`, `maxRequestedWindow = 8`, file-lane weights, and MicroFlowGroup policy defaults. The global budget is the scheduler-level abstraction of the tested binary-v1 outgoing runtime capacity. | A future outer resource arbiter should derive an effective data budget of `8` or `7` before calling the planner. | Do not create a separate ninth capacity unit, reinterpret the existing `control_text` model lane as a transport, or put control events into the current file queue. |
| `src/lib/transferPlanner.ts::planWeightedTransfers` | Pure planner that reserves active requested windows, selects runnable work, distributes the global budget, and ensures requested-window totals do not exceed the budget. | Safe data-allocation boundary after a separate control backlog decision has produced the effective data budget. | Do not add control dispatch, transport, peer execution, or control-event parsing here. |
| `src/lib/transferPlanner.ts::computeLaneBudgets` and `classifyLane` | Reports/model-level lane classification; `text`, `control`, `agent`, and `command` classify to `control_text`. Current queue adapters produce only file-like tasks. | Useful only as historical/model context. A dedicated control lane should have separate semantics and queueing. | Do not treat lane weight `control_text = 1` as the proposed reservation; current allocation remains demand- and sender-task-based. |
| `src-tauri/src/transfer_tuning.rs::DEFAULT_BINARY_V1_WINDOW` | Defines the tested lower-level binary-v1 sender pipeline capacity basis as `8`; supported values clamp from `1` to `16`. The planner's default global budget was set to match this basis. | Confirms the lower-level capacity represented by the scheduler budget. It is not a separate hidden pool for data in addition to planner capacity. | Do not make the binary-v1 hot path own the control lane or change defaults, override precedence, or binary-v1 tuning in the feasibility phase. |
| `src-tauri/src/transfer.rs::send_binary_chunks_pipelined` and `current_runtime_window` | Enforces planner-selected or default runtime windows for outgoing binary-v1 chunks. | Existing mechanism remains the lower-level enforcement path for outgoing data allocations after the planner/resource arbiter exposes `8` or `7` data windows. | Do not insert control events into the binary-v1 file hot path or treat its default window as capacity in addition to the planner budget. |
| `src-tauri/src/transfer.rs::update_active_transfer_window` | Updates supported active outgoing binary-v1 sender windows and rejects receiver, unsupported-protocol, and override-controlled cases. | Shows current runtime mutation is sender-only. A later data reservation may reuse the existing update path only to make outgoing data yield. | Do not change receiver behavior, protocol behavior, or override precedence for this design. |

The current source therefore exposes two layers of the same logical outgoing
runtime-window capacity model:

1. the binary-v1 default runtime window of eight is the tested lower-level
   capacity basis; and
2. the frontend global requested-window budget of eight is the scheduler-level
   abstraction set to match that basis across planned outgoing file-like work.

These values must not be added together or treated as independent pools. The
future control lane reserves from the scheduler-level budget, reducing the data
planner's available capacity from `8` to `7` while control backlog exists. The
binary-v1 hot path remains the lower-level data enforcement mechanism and does
not own the control lane.

### Planner, Queue, and Dispatch

| File / function | Current role | Future control-lane relevance | Avoid or preserve |
| --- | --- | --- | --- |
| `src/lib/transferScheduler.ts::planRunnableTransferLaunches` | Adapts frontend file queue items and room state into planner tasks, calls `planWeightedTransfers`, and exposes ordinary and MicroFlowGroup launch plans. | A future outer resource arbiter could pass a policy with an effective data budget. | Keep the adapter file-only unless a later implementation deliberately creates a separate control scheduler. |
| `src/lib/transferScheduler.ts::planActiveTransferWindowRebalances` | Produces sender-side window changes for active outgoing file transfers. | Potential future mechanism for making already-active data yield after control backlog appears, subject to debug-override and timing rules. | Do not use it as control transport or mutate it during this feasibility phase. |
| `src/App.tsx` planner effect | Calls `planRunnableTransferLaunches`, records launch reservations, and dispatches ordinary file sends or one serial MicroFlowGroup. | A future resource-arbiter integration point could supply the effective data budget and trigger data-window rebalance. | Do not put control events in `launchingQueueItemWindowsRef`, the file queue, or the MicroFlowGroup runner. |
| `src/App.tsx::processTransferQueueItem` | Sends one queued file through `sendFileToRoom` with the planner-requested window. | Data path should continue to consume only data-lane allocation. | Do not route control events through this function. |
| `src/App.tsx::rebalanceActiveTransferWindows` | On file completion/failure/cancel, requests supported sender runtime-window changes from existing planner output. | A later reservation stage may need an additional control-backlog-triggered rebalance policy, after careful validation. | Do not change current completion-triggered behavior in this task. |
| `src/lib/transferScheduler.ts::TransferSchedulerState` | Holds frontend in-memory file queue, batch, cancellation, correlation, and MicroFlowGroup state. | Not the preferred home for semantically separate control queues. A separate control scheduler/state boundary is clearer. | Do not mix control-event state with user file queue state. |

### MicroFlowGroup Interaction

| File / function | Current role | Future control-lane relevance | Avoid or preserve |
| --- | --- | --- | --- |
| `src/lib/transferPlanner.ts::planMicroFlowGroups` and `isMicroFlowGroupEligible` | Groups only eligible file-like queued tasks; control/text/agent/command kinds are not eligible. | Existing file-like eligibility already supports semantic exclusion. | Preserve eligibility, fixed/dynamic behavior, group caps, and one-window accounting. |
| `src/lib/transferPlanner.ts::maxRequestedWindowForCandidate` | Caps a synthetic `micro_group` planner task at one requested window. | A group should continue to consume one of the available data windows after any outer reservation. | Do not make the control lane a group or group child. |
| `src/App.tsx::processMicroFlowGroup` | Runs one selected group serially; every child calls the ordinary single-file queue path. | No direct control-lane hook is needed. | Avoid entirely for control events. |
| `src/lib/transferScheduler.ts` MicroFlowGroup lifecycle helpers | Maintain frontend-only group state and child terminal accounting. | No direct control-lane hook is needed. | Avoid entirely for control events. |

### Text and Room-Item Path

| File / function | Current role | Future control-lane relevance | Avoid or preserve |
| --- | --- | --- | --- |
| `src/pages/RoomPage.tsx::handleSendText` | Sends visible composer text immediately. | None; control events must not use the composer. | Avoid for control transport. |
| `src/lib/tauri.ts::sendTextToRoom` | Invokes the Rust text command. | None. | Avoid for control transport. |
| `src-tauri/src/commands.rs::send_text_to_room` | Creates a persisted outgoing text item and calls `transfer::send_room_item`. | Confirms why control transport needs a separate command/path. | Avoid for control transport. |
| `src-tauri/src/storage.rs::create_outgoing_text_item` | Encrypts and persists `PayloadType::Text` as a room item. | Confirms user-content persistence semantics. | Avoid for control events. |
| `src-tauri/src/transfer.rs::send_room_item` | Sends a stored room item to `POST /rooms/:room_id/items`. | Existing encryption/key handling may inform a later design, but the route and DTO are user-content semantics. | Do not tunnel control events through this function or DTO. |
| `src-tauri/src/transfer.rs::receive_item_handler` | Decrypts received legacy room-item payloads and persists them through `persist_incoming_item`. | Confirms current inbound text/file items are user content. | Do not reinterpret received text as control events. |
| `src-tauri/src/storage.rs::persist_incoming_item` and `room_item_to_info` | Persist incoming room items and render text items as user-visible text. | Confirms control persistence and UI require a separate decision. | Avoid for control events unless a future explicit separate control-event storage model is designed. |

### Current Room Server and AI Slot Preview

| File / function | Current role | Future control-lane relevance | Avoid or preserve |
| --- | --- | --- | --- |
| `src-tauri/src/transfer.rs::start_room_server` | Registers join, room-item, file-transfer, diagnostics, burn, and leave routes. | A future typed bounded room-control route is the clearest transport insertion area found in current architecture. | Do not modify existing item, file, diagnostics, burn, or leave semantics. |
| `src/lib/ai/capabilityPreviewEnvelope.ts` | Defines and validates preview-only capability envelopes, exact fields, unsafe-field rejection, expiry, duplicate checks, and local acknowledge/deny state. | Supplies the specific Hello Peer preview payload wrapped by the CL-1 generic room-control event schema. | Do not treat current preview state as transport security, peer consent, or execution authority. |
| `src/lib/agentBridge/roomControlEvent.ts` | Implements the CL-1 type-only `RoomControlEvent` wrapper, preview/status builders, deny-first validator, current-session duplicate helper, and pure `computeControlLaneBudget` feasibility helper. | Provides the closed preview-only event schema and local validation foundation for later simulation and transport work. | It has no Tauri invoke, room send/receive, transport, scheduler wiring, persistence, or execution authority. |
| `src/lib/agentBridge/controlQueue.ts` | Implements the CL-2 current-session-only local control queue simulation: separate inbound/outbound arrays, priority ordering, duplicate/expiry rejection, strict local terminal transitions, selection, backlog detection, and hypothetical budget calculation. | Validates the proposed shared-lane queue policy before transport or scheduler integration. | It has no persistence, Tauri invoke, room send/receive, scheduler mutation, retry/escalation, runtime result, or execution authority. |
| `src/components/AiSlotPreview.tsx` | Builds outbound envelope previews and exposes the CL-2 local control queue simulation in Developer Tools. It can enqueue local preview/status events, select the next item, show queue state, and show hypothetical data/control budgets. | Reusable UI for local simulation and later visible preview state. | Do not give it direct room-send, scheduler, or executor authority. |
| `docs/agent-bridge/ai-slot-v0-implementation-notes.md` | Records that actual room transport is blocked and text transport was not reused. | Current safety boundary to preserve. | Keep synchronized if a future safe transport is implemented. |

The CL-1 typed room-control event foundation and CL-2 local queue simulation
now exist. The CL-2 queue is current-session frontend state only. No generic
room-control transport, network send/receive, shared inbound/outbound runtime
control-lane scheduler, peer execution path, or capability transport execution
exists.

## 11. Feasibility Conclusion

A one-window logical control reservation appears feasible as a
scheduler/resource-allocation concept. The binary-v1 default runtime window of
`8` is the tested lower-level outgoing capacity basis, and the frontend
planner's `globalWindowBudget = 8` is the scheduler-level abstraction of that
same capacity. They are not independent pools.

The proposed model does not inherently require changing file transfer protocol
semantics. The future planner/resource arbiter should expose all `8` windows to
the data planner when control backlog is empty and `7` when valid control
backlog exists. The reserved control window comes from the same logical
capacity; it is not a ninth lane. Existing outgoing binary-v1 runtime-window
mutation may later help active data yield, but the binary-v1 hot path must not
own or transport the control lane. The control event itself must use a separate
typed room-control transport.

The model does not require changing MicroFlowGroup. MicroFlowGroup should
continue to plan and dispatch only data work within whatever data budget is
available.

The safest future insertion points are:

1. a new dedicated control-event type/validator and separate in-memory
   inbound/outbound control queues;
2. a new bounded typed room-control transport adjacent to, but distinct from,
   the current `/rooms/:room_id/items` and file-transfer routes;
3. a small outer resource arbiter at the planner boundary that computes
   `dataWindowBudget = 7 | 8` from the existing eight-window capacity before
   the file planner runs;
4. only later, a carefully scoped trigger for supported active outgoing data
   transfers to yield when control backlog appears.

With CL-1 types/validators and CL-2 local queue simulation implemented, the
minimum remaining path is a safe typed room-control route for preview events,
followed later by scheduler reservation. The local CL-2 budget calculation is
evidence for the planner/resource-arbiter design; it does not change current
scheduler behavior.

Remaining risks include:

- designing authenticated room-control transport without tunneling through
  user text;
- defining whether and how control events persist;
- binding room, peer identity, and current session;
- preventing replay, spam, malformed traffic, and reservation denial of
  service;
- defining byte, queue, rate, retry, and expiry caps;
- coordinating active data-window yielding when debug overrides are active;
- making a shared inbound/outbound logical lane precise even though current
  inbound work is outside the sender-side planner;
- keeping preview acknowledgment distinct from peer consent and execution
  authorization.

Expected conclusion: a control lane appears feasible as a
scheduler-level/resource-allocation concept, but actual transport requires a
separate room-control event path; it must not reuse `sendTextToRoom`.

## 12. Staged Implementation Status

### Stage CL-0: Docs and Feasibility

Complete. Records the semantic boundary, source-grounded feasibility, capacity
model, risks, and future insertion points without runtime changes.

### Stage CL-1: Type-Only Control Events

Implemented as a frontend/library-only foundation in
`src/lib/agentBridge/roomControlEvent.ts`. It defines the closed preview-only
event kinds, exact-field and unsafe-field validation, bounded identifiers and
status reasons, a `64 KiB` serialized event cap, expiry and expected
room/source/target checks, current-session duplicate helpers, and the pure
`computeControlLaneBudget` feasibility helper.

`computeControlLaneBudget` mirrors the proposed scheduler-level capacity split:
data receives `8` and control `0` without backlog; data receives `7` and
control at most `1` with backlog. It is not wired into the scheduler. CL-1
clamps a supplied invalid total-window value to a positive integer and defaults
to `8`. It performs no transport, room send/receive, persistence, reservation,
or execution.

### Stage CL-2: Local Outbound/Inbound Simulation

Implemented in `src/lib/agentBridge/controlQueue.ts`, with a Developer Tools
preview in `src/components/AiSlotPreview.tsx`. It simulates separate outbound
and inbound current-session queues that share one local selection policy.
Lower priority numbers win: inbound deny/invalid/expired events precede inbound
ack/status events, which precede outbound preview events. FIFO order breaks
ties.

The simulation validates events through CL-1, rejects duplicate/replayed and
already-expired events, marks queued items expired before selection, applies
strict local-only status transitions, and computes the future budget impact:
data `8` / control `0` without backlog and data `7` / control `1` with
backlog. Acknowledgement is `acknowledged_preview_only`, not execution consent.

CL-2 does not send or receive room events, provide room-control transport,
persist queue state, reserve a scheduler window, mutate transfer or
MicroFlowGroup behavior, or produce stdout, stderr, exit codes, or runtime
results.

### Stage CL-3A: Room-Control Transport Design and Feasibility

Complete in `docs/agent-bridge/room-control-transport-design.md`. The
repository-grounded recommendation is one separate bounded
`POST /rooms/:room_id/control-events` route carrying a domain-separated
encrypted current-session control envelope. Existing room HTTP requests are
plain HTTP, so the future route must not assume that the connection itself is
authenticated or encrypted. CL-3A changes no runtime behavior.

### Stage CL-3B: Minimal Preview-Only Room-Control Transport

Implemented for the five preview event kinds through the separate bounded
`POST /rooms/:room_id/control-events` route. It uses current-session
peer-key/source/target binding, control-specific domain-separated key wrapping,
encrypted delivery receipts, bounded in-memory inbox/replay/rate state, narrow
Tauri wrappers, and sanitized Developer Tools visibility. It has no automatic
retry and does not use user text, legacy room-item, file-transfer, or
MicroFlowGroup paths.

### Stage CL-3C: Delivery/Status Integration With Local Control Queues - Future

Connect transport acceptance and delivery state to CL-2 inbound/outbound
control queues. Keep transport delivery receipt separate from
`capability_preview_ack`, peer consent, and execution authorization.

### Stage CL-4: Scheduler Reservation - Future

Add an outer resource arbiter that reserves at most one logical control
window-equivalent from the existing eight-window scheduler abstraction when
valid control backlog exists. Data may borrow all eight when idle; the data
planner receives seven while control backlog exists. This does not create a
ninth lane or make the binary-v1 hot path responsible for control scheduling.
MicroFlowGroup does not change except that it plans within available data
windows.

### Stage CL-5: Peer PolicyGate and Explicit Consent - Future

Design and implement peer-side policy and explicit one-time consent. Trusted
room membership and preview acknowledgement remain insufficient.

### Stage CL-6: Bounded Hello Peer Executor - Future

Only after CL-5, consider the separately reviewed fixed-template bounded Hello
Peer executor. This stage must still reject raw shell, arbitrary code, file
bytes, unbounded output, and hidden transfer.

## 13. Open Questions

- Should control events be persisted?
- If persisted, how does this coexist with low-trace and no-hidden-history
  expectations?
- Should control events appear in normal room history or a separate control
  panel?
- Does CL-3B approve the recommended bounded encrypted room HTTP endpoint, or
  require a stronger room-identity redesign first?
- Is binary-v2 eventually required for clean control frames?
- How are peer identity and current session bound?
- What payload byte caps are acceptable for each event kind?
- What queue-length, event-rate, retry, and expiry caps are acceptable?
- How should backpressure work if control-event spam occurs?
- Which events qualify as valid backlog before reserving data capacity?
- How should active data yield when an env or Developer Tools window override
  is active?
- How should fairness prevent repeated inbound status refreshes from starving
  user-confirmed outbound events?
- Should the control lane exist when Agent Bridge is disabled?
- Should users be able to disable the control lane entirely?
- Does disabling the lane reject remote control events, acknowledge them as
  unsupported, or expose only local status?
- What visible audit/state surface is required without storing raw payload
  history?

## 14. Non-Goals

- no raw shell;
- no arbitrary code execution;
- no peer filesystem search;
- no file transfer over the control lane;
- no hidden transfer;
- no AI-driven scheduler mutation;
- no AI-driven MicroFlowGroup mutation;
- no peer executor;
- no runtime launch;
- no process spawn;
- no stdout or stderr streaming;
- no raw logs or secrets;
- no room text tunneling;
- no reinterpretation of user text;
- no binary-v1 behavior change;
- no binary-v2 implementation in this task;
- no transfer-protocol change;
- no runtime scheduler change in this task.
