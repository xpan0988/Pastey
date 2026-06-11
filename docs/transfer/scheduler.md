# Transfer Scheduler

This is the active source of truth for Pastey's frontend transfer scheduler, weighted planner, and selectable live MicroFlowGroup modes. For the transport overview, see [architecture.md](architecture.md). For validation commands and log interpretation, see [validation.md](validation.md).

## Planner Ownership

The scheduler is frontend-owned and applies only to queued file-like work: file picker transfers, drag/drop transfers, and pasted images that have been written to a temporary file and queued. Text still sends immediately through the text path and does not enter the file queue.

Every ordinary runnable plan still calls the existing `sendFileToRoom` frontend wrapper and Rust `send_file_to_room` command for one file. Scheduler decisions do not create a second transfer core.

## Weighted Planner Model

The pure planner classifies nonterminal queued and active tasks, then returns deterministic runnable, active, held, lane-budget, requested-window, and debug reports.

The global planner budget is a binary-v1 requested-window budget. The default `globalWindowBudget` is `8`; each selected runnable or active file-like transfer receives at least window `1`, and the sum of active plus runnable requested windows must not exceed the global budget.

Lane and size class metadata still matter for classification, priority, eligibility, and reports, but the current file-like requested-window split is batch-relative and size-weighted across selected transfers rather than mostly lane-budget driven. Examples:

- one huge file can receive window `8`;
- huge plus small can receive about `7 + 1`;
- similarly large files split the global budget fairly;
- active transfers reserve their existing requested window before queued work starts.

The safety active-transfer cap is a guardrail, not the main strategy.

## Policy Defaults

Current planner defaults:

- `globalWindowBudget = 8`
- `minRequestedWindow = 1`
- `maxRequestedWindow = 8`
- `safetyActiveTransferCap = 4`
- persisted Release 2.0 `micro_flow_group_mode = "dynamic"`; invalid or missing values normalize to `dynamic`
- `microGroupMaxChildSizeBytes = 1 MiB`
- `microGroupMaxGroupBytes = 4 MiB`
- `microGroupMaxGroupItems = 32`
- lane weights: `control_text = 1`, `small_file = 1`, `bulk_file = 7`

Final sender window precedence stays:

```text
PASTEY_TRANSFER_WINDOW_SIZE env override
  -> effective Developer Tools transfer-window override
  -> planner requestedWindow
  -> default binary-v1 window 8
```

Env and Developer Tools overrides are debugging authorities and override planner requests.

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

Dynamic mode is the Release 2.0 live one-window service policy:

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

Changing `MicroFlowGroup mode` in Developer Tools affects the next planner cycle only. Active ordinary transfers keep their current state except for the existing Phase 4A completion-triggered runtime-window mechanism. Active groups are not regrouped, running children are not relaunched, and grouped-child reservations continue to prevent duplicate launches.

Persistent planner diagnostics identify the actual live mode with `micro_group_mode=fixed|dynamic`. Live fields include `micro_group_plans`, `micro_group_grouped_children`, `micro_group_skip_reason`, `eligible_micro_group_children`, `one_window_quantum_bytes`, `dynamic_child_cap_bytes`, and `dynamic_group_cap_bytes`. Optional comparisons use candidate names such as `fixed_candidate_children` and `dynamic_candidate_children`; dynamic shadow is no longer an active mode.

## Future Extensions

Deficit-mode scheduling, broader adaptive rebalance policy, speed-history heuristics, binary-v2 substreams, and command/agent/control lanes remain non-current. Future text/control/agent/command lanes may be modeled as planner concepts, but they must not be dispatched until a later implementation explicitly defines the authority model and transport path.
