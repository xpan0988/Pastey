# Layer 3 — Smart orchestration

Layer 3 owns transfer planner/scheduler policy, queue lifecycle, capacity accounting, runtime-window allocation, and `MicroFlowGroup`. The ordinary UI queue remains frontend-owned, while a shared Rust capacity-admission boundary applies the same global window and active-transfer limits to both ordinary and managed Transfers before either reaches the existing Rust transfer path. It does not create a second transfer core.

## Planner and capacity

The frontend planner (`src/lib/transferPlanner.ts`) and scheduler (`src/lib/transferScheduler.ts`) plan ordinary queued file-like work. File picker, drag/drop, and pasted-image transfers enter this queue; text uses the immediate text path. Every runnable file-like item eventually calls the existing `sendFileToRoom` wrapper and Rust `send_file_to_room` command for a single selected peer. Rust then admits the bounded resource request through `transfer_orchestration` using only opaque transfer/room identifiers, size, origin, and requested window; private paths, ObjectRefs, and Plan authority are not planner inputs.

The tested global binary-v1 window budget is 8. The shared Rust boundary limits concurrent admitted transfers and reserves effective windows from the same budget for ordinary and managed work. Each selected runnable or active transfer has at least one window, and active/reserved work constrains later launches. Ordinary frontend allocation remains size-weighted within the global budget. Debugging overrides—`PASTEY_TRANSFER_WINDOW_SIZE` and the Developer Tools setting—retain their existing precedence over planner requests.

Outgoing local Bridge control demand reserves capacity by lowering the data target from 8 to 7. After 750 ms of quiet it returns to 8. Existing supported senders hot-adjust through the Layer 1 runtime-window primitive without a cancel/restart. Inbound-only review does not reserve this outgoing capacity.

## Queue lifecycle and multi-target work

Layer 4 resolves selected-peers or explicit ordinary-data broadcast before enqueue. Each file/image/pasted-image target becomes an ordinary target-specific queue child with a shared in-memory `bridgeOperationId`. Child terminal states produce aggregate completed, partial, failed, cancelled, or interrupted presentation. An old route fails for that child and never silently rebinds after reconnect.

The durable Bridge Plan Search → Transfer workflow uses Plan-specific semantic admission: the receiver validates the selected bounded candidate and resolves its source inside Rust. A Layer-5 Host continuation coordinator claims only the next dependency-eligible authored Transfer; the shared Layer-3 Rust capacity boundary then decides its transport window before Layer 1 sends bytes. The frontend scheduler does not receive a path-bearing Transfer item and is not required for managed continuation. `handoff_queued` means Rust accepted the transfer operation, not that bytes moved or the transfer completed.

Layer 3 does not retain Layer 5 Transform authority or result metadata. ObjectRefs, private sources, consent IDs, candidate IDs, previews, approval records, leases, resolved intents, implementation identities, request hashes, and other Layer 5 authority data stay out of Layer 3.

## MicroFlowGroup

`MicroFlowGroup` is a scheduler resource abstraction for eligible tiny queued payloads. A synthetic planner task receives one window, while the group runner sends children serially through the ordinary single-file path. It is not a bundle, archive, protocol object, Bridge item, binary-v2 stream, remote execution object, or permission grant.

The persisted default is `dynamic`; `fixed` remains available as a Developer Tools fallback. Both preserve per-child transfer accounting. Dynamic grouping occurs under contention, permits one active dynamic group, uses bounded service cost and group limits, and does not regroup running children. Fixed mode requires at least two compatible queued file-like children within the configured item, byte, and count limits.

## Boundaries

Layer 2 provides facts, not planner commands. Layer 3 owns resource policy over queue state, bounded transfer size/origin, control workload, runtime capacity, and terminal state. Layer 4 owns current-session peer identity, routing, and control delivery. Layer 5 owns Plan topology, semantic eligibility, approval, ObjectRef/private-path authority, Host binding, and exact step authority.

The canonical dependency is `Layer 5 eligibility → Layer 3 capacity → Layer 1 byte transfer`, with Layer 4 supplying current authenticated/session transport context where required. Layer 3 cannot create or reorder Plan steps, insert movement, or turn capacity admission into semantic authority.

For the underlying byte transfer, see [Layer 1](layer-1-transfer.md). For runnable commands and contention/smoke evidence, see [development.md](../development.md).
