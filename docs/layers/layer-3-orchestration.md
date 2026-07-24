# Layer 3 — Smart orchestration

Layer 3 owns the frontend transfer planner, scheduler policy, queue lifecycle, capacity accounting, runtime-window allocation, and `MicroFlowGroup`. It reuses the existing Rust transfer path; it does not create a second transfer core.

## Planner and capacity

The frontend planner (`src/lib/transferPlanner.ts`) and scheduler (`src/lib/transferScheduler.ts`) plan queued file-like work. File picker, drag/drop, and pasted-image transfers enter this queue; text uses the immediate text path. Every runnable file-like item eventually calls the existing `sendFileToRoom` wrapper and Rust `send_file_to_room` command for a single selected peer.

The tested global binary-v1 window budget is 8. Each selected runnable or active transfer has at least one window, and active/reserved work constrains later launches. Allocation is size-weighted within the global budget. Debugging authorities—`PASTEY_TRANSFER_WINDOW_SIZE` and the Developer Tools override—take precedence over planner requests.

Outgoing local Bridge control demand reserves capacity by lowering the data target from 8 to 7. After 750 ms of quiet it returns to 8. Existing supported senders hot-adjust through the Layer 1 runtime-window primitive without a cancel/restart. Inbound-only review does not reserve this outgoing capacity.

## Queue lifecycle and multi-target work

Layer 4 resolves selected-peers or explicit ordinary-data broadcast before enqueue. Each file/image/pasted-image target becomes an ordinary target-specific queue child with a shared in-memory `bridgeOperationId`. Child terminal states produce aggregate completed, partial, failed, cancelled, or interrupted presentation. An old route fails for that child and never silently rebinds after reconnect.

The durable Bridge Plan Search → Transfer workflow uses Plan-specific admission: the receiver validates the selected bounded candidate and resolves its source inside Rust before handing it to the transfer engine. The frontend scheduler does not receive a path-bearing Transfer item. `handoff_queued` means Rust accepted the transfer operation, not that bytes moved or the transfer completed.

Layer 3 does not retain Layer 5 Transform authority or result metadata. ObjectRefs, private sources, consent IDs, candidate IDs, previews, approval records, leases, resolved intents, implementation identities, request hashes, and other Layer 5 authority data stay out of Layer 3.

## MicroFlowGroup

`MicroFlowGroup` is a scheduler resource abstraction for eligible tiny queued payloads. A synthetic planner task receives one window, while the group runner sends children serially through the ordinary single-file path. It is not a bundle, archive, protocol object, Bridge item, binary-v2 stream, remote execution object, or permission grant.

The persisted default is `dynamic`; `fixed` remains available as a Developer Tools fallback. Both preserve per-child transfer accounting. Dynamic grouping occurs under contention, permits one active dynamic group, uses bounded service cost and group limits, and does not regroup running children. Fixed mode requires at least two compatible queued file-like children within the configured item, byte, and count limits.

## Boundaries

Layer 2 provides facts, not planner commands. Layer 3 owns the policy that uses explicit user intent, queue state, payload size, control workload, runtime capacity, and terminal state. Layer 4 owns peer identity and routing. Layer 5 owns consent and capability authority.

For the underlying byte transfer, see [Layer 1](layer-1-transfer.md). For runnable commands and contention/smoke evidence, see [development.md](../development.md).
