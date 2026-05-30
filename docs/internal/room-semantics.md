# Room Semantics

## Purpose

This document defines what a Room means inside Pastey.

A Room is a local-first coordination space between trusted devices. It exists to coordinate transfers, approvals, activity, and future device capability workflows.

A Room is not:

- a cloud drive
- a cloud storage bucket
- a permanent account-backed workspace
- a global chat room
- a remote filesystem
- an implicit broadcast disk
- an unrestricted execution context

## Non-goals

A Room SHOULD NOT silently upload data to cloud services.

A Room SHOULD NOT imply that every payload is copied to every peer.

A Room MUST NOT grant command execution authority by itself.

A Room SHOULD NOT behave like a shared network drive unless that behavior is explicitly designed later.

## Core Entities

Room: A local coordination object for trusted devices, room items, transfers, approvals, activity, and capability metadata.

Room ID: A local identifier for a room instance. It is used to associate local state, peer coordination, activity, and transfers.

Device: A Pastey runtime instance on a physical or virtual machine. A device has local identity, profile metadata, settings, and local storage.

Peer: A trusted device that is connected to or participating in a room from the perspective of the local device.

Room Item: A conceptual item visible in a room, such as text, a lightweight image payload, or a file offer. A room item is not necessarily equivalent to a copied file on every device.

Transfer: A concrete movement of payload data between devices. A transfer has sender and receiver state and may complete, fail, be cancelled, be interrupted, or be burned.

Inbox Item: User-owned output saved after successful receipt according to Inbox settings. Inbox items have a lifecycle separate from the room that delivered them.

Activity Event: A user- or system-relevant fact about room coordination, such as a device joining, a file being offered, a transfer completing, or a burn occurring.

Capability Metadata: Advisory device information such as profile, runtime, CPU, GPU, power, and benchmark signals. Capability metadata is not a permission grant.

## Room Lifecycle

Room lifecycle states are conceptual:

- created
- active
- closing / burning
- burned
- interrupted
- archived, future/optional

A Room SHOULD NOT be destroyed only because a default expiry duration elapsed.

Manual Burn is the authoritative destructive transition for the local room instance.

Burn SHOULD be terminal for that local room instance.

A burned room MUST reject new joins, new items, and new transfers.

Pending transfers SHOULD be cancelled or marked interrupted during Burn.

Startup recovery MAY mark stale active rooms or transfers as interrupted depending on implementation.

Current rooms are manual-burn lifecycle objects in-session. Full reconnectable persistent rooms across app restart are future work unless explicitly implemented.

## Burn Semantics

Burn is a terminal room state transition. It stops active coordination, removes transient room data, cancels or interrupts pending transfers, removes temporary payloads and `.part` state where applicable, and emits or records activity where supported.

Burn MUST NOT silently delete user-owned Inbox-saved files or images.

## Room Lifecycle vs Content Lifecycle

Room lifecycle controls:

- coordination state
- room metadata
- transient payloads
- pending transfer state
- active receiver/sender state

Content lifecycle controls:

- whether received files/images are preserved in Inbox
- whether transient received items are cleaned
- user-owned output after successful receipt

Inbox-saved files and images are user-owned output.

Burning a Room MUST NOT be treated as equivalent to deleting all files ever received through that Room.

Transient received items MAY be cleaned during Burn or recovery.

Finalized saved Inbox outputs SHOULD remain unless the user explicitly deletes them.

## Payload Routing Semantics

Text messages and lightweight image payloads MAY be room-broadcast content.

File payloads SHOULD be represented as file offers.

Large files MUST NOT be automatically broadcast to every room peer by default.

File transfers SHOULD be targeted to one or more selected destination devices.

Current Global Transfer Scheduler v1 is local frontend orchestration for outbound file sends. It may queue multiple selected or dropped files, start multiple existing queued file-like transfers according to weighted planner output, and update active outgoing binary-v1 sender windows after planner-managed queue item completion. Each runnable plan still uses the existing single-file transfer path. It MUST NOT imply archive bundling, folder recursion, retry/timeout adaptive windows, history-aware tuning, protocol changes, or a second backend transfer core.

File type MAY affect display labels. It MUST NOT change the core file transport behavior.

"Send to all" MAY exist in the future, but it should be explicit and show cost/impact.

For example, a `.gguf` model file should appear as an offer or activity visible in the Room, but only selected devices such as a desktop GPU machine or NAS should actually download it.

## Multi-device Room Direction

Rooms are intended to support more than two devices.

Future distributed rooms should avoid assuming a permanent single sender/receiver pair.

Room state should move toward event-driven semantics.

A Room should have terminal states to prevent orphaned coordination state, leaks, or ambiguous cleanup.

Future distributed state may be represented through a `RoomEvent` log.

Fully distributed multi-device rooms are future work and are not implied by the current implementation.

## Room Events

Candidate room event types include:

- `RoomCreated`
- `DeviceJoined`
- `DeviceLeft`
- `RoomBurned`
- `TextBroadcasted`
- `ImageBroadcasted`
- `FileOffered`
- `TransferAccepted`
- `TransferStarted`
- `TransferCompleted`
- `TransferFailed`
- `ApprovalRequested`
- `ApprovalGranted`
- `CapabilityAdvertised`

Room state SHOULD eventually be derivable from an append-only event history where possible.

## Trust and Authority

Joining a room does not automatically grant unrestricted device authority.

Trust is explicit and revocable.

Capabilities are advertised metadata, not permission grants.

Execution or sensitive actions require explicit approval in future capability workflows.

Diagnostics are advisory signals only.

## Diagnostics and Capability Relationship

`DeviceProfile` and capability probing describe a device.

Link benchmarks estimate routing suitability.

These signals MAY influence recommendations.

They MUST NOT by themselves authorize file access, command execution, or agent actions.

`recommended_roles` are internal hints, not permissions.

## Current Implementation Notes

Current rooms support local-first transfer coordination.

Current lifecycle is manual-burn based in-session.

Received files and images can be saved to Inbox according to settings.

Burn preserves Inbox-saved outputs.

Multi-device distributed room semantics are future work.

## Design Principles

- Local-first by default
- Explicit trust
- Burn is terminal
- Inbox output is user-owned
- Lightweight content can be room-oriented
- Large files are targeted
- Diagnostics inform but do not authorize
- AI/Agent support is optional and future-facing
