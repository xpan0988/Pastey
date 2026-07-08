# Transfer Scheduler

This is the active source of truth for Pastey's Layer 3 transfer scheduler, weighted planner, runtime-window allocation, and live MicroFlowGroup behavior. For transport mechanics, see [architecture.md](architecture.md). For validation, see [validation.md](validation.md). For the project-wide layer contract, see [../architecture/Project-specifications.md](../architecture/Project-specifications.md). For Bridge terminology, see [../architecture/bridge-semantics.md](../architecture/bridge-semantics.md).

## Planner Ownership

The scheduler is frontend-owned and applies to queued file-like work: file picker transfers, drag/drop transfers, and pasted images that have been written to a temporary file and queued. Text still sends immediately through the text path and does not enter the file queue.

Every ordinary runnable plan still calls the existing `sendFileToRoom` frontend wrapper and Rust `send_file_to_room` command for one file. These are legacy implementation names for Bridge file transfer. Scheduler decisions do not create a second transfer core.

Multi-target ordinary file-like sends are represented as queue children, not as a second transfer core. A selected-peers or explicit broadcast file/image/pasted-image action resolves the current routeable target set before enqueue and creates one ordinary queue item per target. The children share one in-memory `bridgeOperationId`, carry target display/session refs for UI accounting, and each child carries a selected-peer route for dispatch. These fields are current-session runtime state; they are not durable route metadata, durable trust, or reusable consent.

Production paths:

- Pure weighted planner: `src/lib/transferPlanner.ts`.
- Scheduler adapter and lifecycle planning: `src/lib/transferScheduler.ts`.
- App dispatch and runtime rebalance effects: `src/App.tsx`.
- Runtime-window bridge: `src/lib/tauri.ts`.
- Rust active sender update primitive: `src-tauri/src/transfer.rs`.
- Control-demand reducer: `src/lib/agentBridge/controlWindowRuntime.ts`.

## Weighted Planner Model

The pure planner classifies queued and active small-payload/data tasks, then returns deterministic runnable, active, held, lane-budget, requested-window, and debug reports.

The global planner budget represents tested outgoing binary-v1 runtime capacity. The normal data target is `8`. Real local outgoing Bridge control demand changes the effective data target to `7`, then restores `8` after the quiet period. Each selected runnable or active transfer receives at least window `1`, and the sum of active plus runnable requested windows must not exceed the current data target.

Lane and payload byte-size facts affect classification, priority, eligibility, and diagnostics. MIME family, file extension, and receiver file-format concerns remain metadata/UI hints and do not affect MicroFlowGroup grouping or requested-window allocation. The requested-window split is batch-relative and size-weighted across selected transfers.

Examples:

- one huge file can receive window `8`;
- huge plus small can receive about `7 + 1`;
- similarly large files split the global budget fairly;
- active transfers reserve their existing requested window before queued work starts.

Several outgoing binary-v1 transfers may be active at once. The planner recomputes their combined allocation within the current target; control demand never assigns window `7` independently to every active transfer.

## Policy Defaults

Current planner defaults:

- `globalWindowBudget = 8`
- `minRequestedWindow = 1`
- `maxRequestedWindow = 8`
- `safetyActiveTransferCap = 4`
- persisted `micro_flow_group_mode = "dynamic"` by default
- invalid or missing MicroFlowGroup mode normalizes to `dynamic`
- `microGroupMaxChildSizeBytes = 1 MiB`
- `microGroupMaxGroupBytes = 4 MiB`
- `microGroupMaxGroupItems = 32`
- lane weights: `control_text = 1`, `small_file = 1`, `bulk_file = 7`

Runtime control-demand policy:

- outgoing Bridge control transport demand: effective data target `7`;
- inbound-only control review state: effective data target remains `8`;
- idle restoration quiet period: `750 ms`;
- new launches use the current target;
- existing supported active binary-v1 senders hot-adjust through `update_transfer_window` without cancellation or restart.

Env and Developer Tools transfer-window overrides are debugging authorities and override planner requests.

## Live MicroFlowGroup Modes

`MicroFlowGroup` is a scheduler-level resource abstraction. It lets several eligible tiny payload queue items share one logical planner window while preserving each child as an ordinary single-file transfer.

Both `fixed` and `dynamic` modes use the same serial execution path:

- the pure planner emits `microGroupPlans` alongside ordinary reports;
- grouped children are replaced by one synthetic planner task with `kind = "micro_group"` for allocation;
- the synthetic group consumes exactly one requested window;
- `planRunnableTransferLaunches` exposes serial group launch plans separately from ordinary runnable plans;
- `App.tsx` sends each child through the existing single-file path with the group requested window;
- group runtime state is frontend bookkeeping only.

Fixed mode preserves the threshold-based fallback behavior:

- queued only, not already preparing or sending;
- metadata-ready with known size;
- active Bridge session only;
- non-cancelled and nonterminal;
- payload-backed queue kinds only: `file`, `image`, and `pasted_image`;
- default child size no larger than `1 MiB`;
- default total group size no larger than `4 MiB`;
- default item count no more than `32`;
- at least two eligible children;
- same Bridge session, lane, size class, and payload-like scheduler class.

Dynamic mode is the current default live one-window service policy:

- grouping occurs only under contention;
- service cost is payload bytes plus `256 KiB` per file;
- one-window quantum is clamped from `4 MiB` to `16 MiB`;
- child cap is clamped from `1 MiB` to `4 MiB`;
- group cap is clamped from `4 MiB` to `16 MiB`;
- groups require at least two children and contain at most `32` children;
- at most one dynamic MicroFlowGroup window is active;
- smaller candidates are selected deterministically to make useful use of the one-window service quantum.

Changing the persisted Settings mode affects the next planner cycle only. Active ordinary transfers may receive completion-triggered or control-demand-triggered runtime-window updates. Active groups are not regrouped, running children are not relaunched, and grouped-child reservations prevent duplicate launches.

`MicroFlowGroup` is not a bundle, archive, zip, Bridge item, protocol object, binary-v2 stream, remote execution object, or permission grant.

## Group Lifecycle

The serial group runner records internal lifecycle state for diagnostics and tests:

- `queued`: a serial group launch plan has been selected and recorded;
- `running`: the serial child loop has started;
- `completed`: every planned child completed;
- `completed_with_errors`: at least one child failed or was individually cancelled, or the group finished with unaccounted children without a batch/room terminal reason;
- `cancelled`: the group was stopped by batch cancellation, or all children were cancelled;
- `interrupted`: Bridge work was cleared or burned while the group was queued/running.

Child terminal accounting is per queue item id. A child failure does not corrupt or revive other children, and late child progress still uses terminal queue item guards.

## Bridge Target Children

For multi-target file-like sends, the parent operation is derived from children with the same `bridgeOperationId`.

- child `completed` means that target's selected-peer transfer completed;
- child `failed` means that target failed without changing other children;
- child `cancelled` means that target was cancelled by item or batch cancellation;
- if a child carries an old selected-peer route that becomes stale, expired, disconnected, reconnecting, or otherwise unrouteable before dispatch, the child fails with the route-specific error and does not rebind to a reconnected peer row;
- room burn/clear paths keep existing terminal guards and do not revive children;
- aggregate UI state is completed when all children complete, partial when some complete and some fail/cancel, failed when no child completes, and cancelled/interrupted according to existing batch and room terminal paths.

Target-aware dedupe keys allow the same file path to create one child per peer without collapsing multi-target sends into one item. The planner still sees ordinary queue items and applies the same global window budget, active-transfer cap, MicroFlowGroup rules, burn guards, cancellation guards, and terminal-item guards. The scheduler does not assign a separate unbounded transfer window per target.

There is no automatic target-specific reconnect retry in the scheduler. A user can enqueue a new send against the current routeable peer set, which creates new current-session children and a new operation id. Existing failed or terminal children are not revived by reconnect.

## Device Intelligence Boundary

Layer 2 provides current-session facts: device profile fields, capability availability, benchmark measurements, liveness, and endpoint/provider availability where implemented. It does not recommend peers, rank devices, produce planner hints, or tell the scheduler what to do.

Layer 3 owns scheduler policy. The current scheduler uses explicit user intent, queue state, data/control class, payload byte size, runtime capacity, control workload, terminal state, and observed runtime transfer facts to select requested windows, choose MicroFlowGroup mode, determine grouping eligibility, rebalance runtime windows, or route transfers. It does not obey Layer 2 planner hints or recommendations.

The current `transfer.request_candidate_payload` path, consolidated in Pastey 1.9.1, validates a Layer 5 second-consent boundary, resolves a selected candidate through a receiver-local in-memory store, and creates a normal Layer 3 queue item through the existing transfer scheduler. The queue item carries Agent Bridge audit metadata such as `origin: agent_bridge_candidate_payload` and the label `Agent Bridge candidate payload request`, but that metadata does not include receiver absolute paths or file contents. `handoff_queued` means the queue accepted the payload source; it does not mean transfer started or completed. Actual byte transfer, runtime-window selection, MicroFlowGroup behavior, progress, cancellation, and completion remain existing scheduler and binary-v1 pipeline responsibilities.

## Validation

Focused automated contention evidence is available through:

```sh
rtk node scripts/run-cl4-contention-smoke.mjs
```

The harness measures the production control-demand reducer and planner allocations, verifies the real Rust active binary-v1 sender runtime-window update primitive, and runs Bridge control transport tests. It is deterministic lower-boundary integration evidence, not a full dual-instance GUI or two-device LAN transfer run.

The Dynamic MicroFlowGroup capacity design reference is retained as [dynamic-microflowgroup-window-capacity-design.pdf](dynamic-microflowgroup-window-capacity-design.pdf). This scheduler document is the active implementation source of truth.

## Non-Current Extensions

Deficit-mode scheduling, broader retry/timeout adaptation, speed-history heuristics, binary-v2 substreams, backend-owned scheduling, and generic command/agent execution lanes are not current production behavior.
