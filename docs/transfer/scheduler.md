# Transfer Scheduler

This is the active source of truth for Pastey's frontend transfer scheduler, weighted planner, and selectable live MicroFlowGroup modes. For the transport overview, see [architecture.md](architecture.md). For validation commands and log interpretation, see [validation.md](validation.md).

## Planner Ownership

The scheduler is frontend-owned and applies only to queued file-like work: file picker transfers, drag/drop transfers, and pasted images that have been written to a temporary file and queued. Text still sends immediately through the text path and does not enter the file queue.

Every ordinary runnable plan still calls the existing `sendFileToRoom` frontend wrapper and Rust `send_file_to_room` command for one file. Scheduler decisions do not create a second transfer core.

## Weighted Planner Model

The pure planner classifies nonterminal queued and active tasks, then returns deterministic runnable, active, held, lane-budget, requested-window, and debug reports.

The global planner budget is the scheduler representation of the tested binary-v1 outgoing runtime capacity. The normal target is `8`. CL-4 changes the effective data target to `7` while real local outgoing room-control work is queued, selected, or sending, then restores `8` after a short quiet period. Each selected runnable or active file-like transfer receives at least window `1`, and the sum of active plus runnable requested windows must not exceed the current data target.

Lane and size class metadata still matter for classification, priority, eligibility, and reports, but the current file-like requested-window split is batch-relative and size-weighted across selected transfers rather than mostly lane-budget driven. Examples:

- one huge file can receive window `8`;
- huge plus small can receive about `7 + 1`;
- similarly large files split the global budget fairly;
- active transfers reserve their existing requested window before queued work starts.

Several outgoing binary-v1 transfers may be active at once. The planner
recomputes their combined allocation within the current target; CL-4 never
assigns window `7` independently to every active transfer.

The safety active-transfer cap is a guardrail, not the main strategy.

## Policy Defaults

Current planner defaults:

- `globalWindowBudget = 8`
- `minRequestedWindow = 1`
- `maxRequestedWindow = 8`
- `safetyActiveTransferCap = 4`
- persisted `micro_flow_group_mode = "dynamic"` by default; invalid or missing values normalize to `dynamic`
- `microGroupMaxChildSizeBytes = 1 MiB`
- `microGroupMaxGroupBytes = 4 MiB`
- `microGroupMaxGroupItems = 32`
- lane weights: `control_text = 1`, `small_file = 1`, `bulk_file = 7`

CL-4 runtime policy:

- outgoing control transport demand: effective data target `7`;
- inbound-only control review state: effective data target remains `8`;
- idle restoration quiet period: `750 ms`;
- new launches use the current target;
- existing supported active binary-v1 senders hot-adjust through
  `update_transfer_window` without cancellation or restart.

Final sender window precedence stays:

```text
PASTEY_TRANSFER_WINDOW_SIZE env override
  -> effective Developer Tools transfer-window override
  -> planner requestedWindow
  -> default binary-v1 window 8
```

Env and Developer Tools overrides are debugging authorities and override planner requests.

Focused automated CL-4 contention evidence is available through
`rtk node scripts/run-cl4-contention-smoke.mjs`. It measures the production
demand/target reducer and planner allocations, then verifies the real Rust
active binary-v1 sender runtime-window update primitive and existing
room-control transport tests. The harness is an in-process/lower-boundary
integration run, not a full dual-instance Tauri or network file-transfer run;
see [validation.md](validation.md) for its exact coverage and limits.

## Live MicroFlowGroup Modes

`MicroFlowGroup`, also called `SparseFlowGroup` in older planning notes, is a scheduler-level resource abstraction. It lets several eligible tiny file-like queue items share one logical planner window while preserving each child as an ordinary single-file transfer.

Both `fixed` and `dynamic` modes use the same serial execution path:

- the pure planner emits `microGroupPlans` alongside ordinary reports;
- grouped children are replaced by one synthetic planner task with `kind = "micro_group"` for allocation;
- the synthetic group consumes exactly one requested window;
- `planRunnableTransferLaunches` exposes serial group launch plans separately from ordinary runnable plans;
- `App.tsx` runs one serial group at a time by sending each child through the existing `processTransferQueueItem` / `sendFileToRoom` path with the group requested window;
- group runtime state is frontend bookkeeping only.

Fixed mode preserves the stable legacy thresholds:

- queued only, not already preparing or sending;
- metadata-ready with known size;
- active room only;
- non-cancelled and nonterminal;
- file-like kinds only: `file`, `image`, and `pasted_image`;
- default child size no larger than `1 MiB`;
- default total group size no larger than `4 MiB`;
- default item count no more than `32`;
- at least two eligible children;
- same room, lane, size class, file-like safety class, and broad MIME family.

A single eligible tiny file remains an ordinary runnable transfer. Files above the fixed child-size limit, such as 1.1 MiB to 1.3 MiB files under the current default, are not fixed-mode MicroFlowGroup children.

Dynamic mode is the current default live one-window service policy:

- grouping occurs only under contention;
- service cost is payload bytes plus `256 KiB` per file;
- one-window quantum is clamped from `4 MiB` to `16 MiB`;
- child cap is clamped from `1 MiB` to `4 MiB`;
- group cap is clamped from `4 MiB` to `16 MiB`;
- groups require at least two children and contain at most `32` children;
- at most one dynamic MicroFlowGroup window is active;
- smaller candidates are selected deterministically to make useful use of the one-window service quantum.

The persisted Settings value defaults to `dynamic`. Invalid or missing values safely normalize to `dynamic`; `fixed` remains available as a fallback and debug baseline.

`MicroFlowGroup` is not a bundle, archive, zip, room item, protocol object, binary-v2 stream, remote execution object, or permission grant. It does not alter child file metadata, payload encryption, binary-v1 frame behavior, ACK behavior, retry behavior, finalize behavior, cancel/burn behavior, or Inbox behavior.

## Group Lifecycle

The serial group runner records internal lifecycle state for diagnostics and tests:

- `queued`: a serial group launch plan has been selected and recorded;
- `running`: the serial child loop has started;
- `completed`: every planned child completed;
- `completed_with_errors`: at least one child failed or was individually cancelled, or the group finished with unaccounted children without a batch/room terminal reason;
- `cancelled`: the group was stopped by batch cancellation, or all children were cancelled;
- `interrupted`: room work was cleared or burned while the group was queued/running.

Child terminal accounting is per queue item id. A child failure does not corrupt or revive other children, and late child progress still uses terminal queue item guards.

## Hot Switching And Diagnostics

Changing `MicroFlowGroup mode` in Developer Tools affects the next planner cycle only. Active ordinary transfers may receive completion-triggered or CL-4 control-demand-triggered runtime-window updates. Active groups are not regrouped, running children are not relaunched, and grouped-child reservations continue to prevent duplicate launches.

Persistent planner diagnostics identify the actual live mode with `micro_group_mode=fixed|dynamic`. Live fields include `micro_group_plans`, `micro_group_grouped_children`, `micro_group_skip_reason`, `eligible_micro_group_children`, `one_window_quantum_bytes`, `dynamic_child_cap_bytes`, and `dynamic_group_cap_bytes`. Optional comparisons use candidate names such as `fixed_candidate_children` and `dynamic_candidate_children`; dynamic shadow is no longer an active mode.

Device Diagnostics is separate from planner diagnostics. The planner does not consume `DeviceProfile`, `DeviceCapabilities`, internal `recommended_roles`, or `LinkBenchmarkResult` values to select requested windows, choose or change MicroFlowGroup mode, determine grouping eligibility, rebalance runtime windows, or route transfers.

## Future Extensions

Deficit-mode scheduling, broader adaptive rebalance policy, speed-history heuristics, binary-v2 substreams, and command/agent execution lanes remain non-current. The implemented control reservation is sender-side capacity policy for the separate bounded room-control transport; it is not an execution lane.
