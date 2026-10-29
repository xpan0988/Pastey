# Pastey reference

This document owns concrete identifiers, bounds, configuration facts, and source pointers. Source types and validators remain authoritative; architecture is in [architecture](architecture.md), [Layer 5](layers/layer-5-agent.md), and [Windows managed execution](platform/windows-managed-execution.md).

## Terms used by current contracts

| Term | Meaning in this implementation |
| --- | --- |
| Host | A device/execution locality with a durable `HostRef`; a remote action additionally requires its exact current Bridge session binding. |
| Capability | A bounded ability observed or exposed by a Host. Observation and compatibility do not select a Host or grant execution authority. |
| Agent | An intelligence/execution participant that can make or propose task decisions over capabilities and resources. Pastey does not currently integrate arbitrary Agents. |
| Native Agent | Pastey's product and protocol family (`native_agent.*`) for invoking native Host capabilities through one capability-neutral lifecycle. |
| Native capability | A Host-owned mature runtime behind that lifecycle, reached through a `NativeCapabilityAdapterV1` adapter; it owns HOW. Codex (`agent.coding.codex`) is the current production adapter. |
| Native executor admission / executor Review | The executing Host's start-time authority gate for a native invocation: Local is admitted at once; remote waits for its user's Accept. |
| Requester movement approval | The requesting Host's approval to move its workspace and request the remote operation. It is not execution authority. |
| Managed Host admission | The managed Plan path's attempt-bound admission (`host-admission:v2-attempt-bound:*`). It is unrelated to native executor admission. |
| Generic Managed Worker | The distinct bounded Worker path for one already authorized Transform or Execute step. It is not the Native Agent lifecycle. |
| Managed execution | The current Plan/ObjectRef/`ManagedObjectRevisionV2` path with Search, Transform, Transfer, and Execute; those are managed primitives rather than a universal capability vocabulary. |

## Versions and schemas

| Boundary | Value / source |
| --- | --- |
| Packaged application metadata | `src-tauri/Cargo.toml`, mirrored by `package.json`, `src-tauri/tauri.conf.json`, and lockfiles |
| Natural-v1 proposal | `ask-bridge-natural-v1` — `src/lib/ai/naturalV1Plan.ts` |
| Natural-v2 candidate | `CandidateSemanticPlanV2` — `src/lib/ai/naturalV2Plan.ts`; Core resolution in `src-tauri/src/natural_v2.rs` |
| Plan v1 | `bridge-plan-v1`; `bridge-plan-revision-hash-v1:*` |
| Plan protocol v1 | `pastey-bridge-plan-protocol-v1` |
| Plan v2 | `bridge-plan-v2`; `bridge-plan-revision-hash-v2:*` |
| Plan protocol v2 | `pastey-bridge-plan-protocol-v2` |
| Product status DTO/event | `pastey-native-v2-product-v1`; `pastey://native-v2-plan-status` |
| Worker status event | `pastey-managed-worker-status-v1`; `pastey://managed-worker-status` |
| Provider config | `pastey-worker-provider-config-v1` |
| Peer capability facts | `pastey-peer-capabilities-v2` |
| Fixed Host system-probe request | `capabilityIds`: at most 12 deduplicated IDs from `runtime.python`, `runtime.node`, `runtime.git`, `runtime.rust_cargo`, `runtime.docker`, `runtime.ffmpeg`, `runtime.cuda`, `runtime.powershell`, `runtime.zsh`, and `runtime.bash`. The receiving Host alone maps an ID to a fixed local probe. Results are `Available`, `Unavailable` (`system_probe_unavailable`), or `Unsupported` (`system_probe_unsupported`); absence is no observation/unknown. No path, command, args, shell, or acquisition fields. |
| Capability-acquisition confirmation | `CapabilityAcquisitionRequestV1` / `confirm_capability_acquisition`: validated durable `HostRef`, one bounded semantic `capabilityId` (`[A-Za-z0-9._:-]`, 1–128 bytes), bounded renderer-safe display text, and only `confirmed` or `cancelled`. Valid semantic acquisition IDs include `runtime.java`, `tool.cmake`, `sdk.android`, and `model.whisper`; membership in the fixed-probe vocabulary is not required. Confirmed means consent to a future Host-side continuation only; it does not install, acquire, probe, bind, authorize, or mutate capability/Plan/topology/Host selection/Developer Mode state. Generic capability acquisition/install behavior is not implemented. Source: `src-tauri/src/capability_acquisition_confirmation.rs`. |
| Bridge NodeList projection | `pastey-bridge-node-list-v1` |
| Room Control route | `pastey-bridge-control-route-v1` |
| Bridge authentication (pairing, reconnect, room requests) | `pastey-bridge-auth-v1` — `src-tauri/src/bridge_auth.rs` |

## Native-v2 commands

Registered Tauri commands:

- `compose_natural_v2_candidate`
- `compose_native_v2_plan`
- `approve_native_v2_plan`
- `start_native_v2_plan_attempt`
- `get_native_v2_plan_status`
- `cancel_native_v2_plan_attempt`

The Natural-v2 candidate plus approve/start/status/cancel commands have TypeScript wrappers in `src/lib/tauri.ts`; the lifecycle wrappers are used by the current opened-revision UI. `compose_native_v2_plan` remains a backend seam without a TypeScript wrapper.

`NativeV2PlanStatusV1` exposes only: schema/Plan/revision/hash, optional approval and attempt ids, state, optional current step, completed/total step counts, ready/total Host counts, bounded code, and update time. It contains no credential, path, ObjectRef, grant, EffectEnvelope, or evidence.

Native-v2 Room Control kinds are:

- `bridge_plan.v2.review_request`
- `bridge_plan.v2.readiness_request` / `bridge_plan.v2.readiness_result`
- `bridge_plan.v2.attempt_start` / `bridge_plan.v2.attempt_prepared` / `bridge_plan.v2.attempt_commit`
- `bridge_plan.v2.step_result` / `bridge_plan.v2.step_failure` / `bridge_plan.v2.step_commit`
- `bridge_plan.v2.attempt_cancel`

Local-Host transitions use `NativeV2CoordinatorActionV1` directly and do not create a route, Room Control event, `HostSessionBinding`, or `session_pair_ref`. Their freshness is one opaque `LocalRuntimeRef` tied to the durable local `HostRef` and current HostRuntime generation. Remote transitions are unchanged: `HostSessionBinding.binding_ref` is the full directional Host-private authority reference, while `HostSessionBinding.session_pair_ref` is the symmetric cross-side correlation for readiness, prepared, step-result, and step-failure payloads. It is derived from the exact Bridge plus both Host/session endpoints, excludes routes, and is not authority.

Native-v2 Host admission is bound to `attempt_id` as well as the exact approval, Plan revision/hash, participant, Host/session binding, TTL, and local authored fragment. Current admission references use `host-admission:v2-attempt-bound:*`; changing an attempt cannot reuse an admission.

The maximum native-v2 approval/attempt lifetime is 24 hours. Identifiers are bounded to 128 characters and product semantic text to 1,024 characters by the native-v2 service.

## Native Agent facts

Native Agent execution, result capture, result Transfer, conflict retention, result Apply, and movement completion are distinct durable facts. A completed Agent task remains `Completed` when a later consequence fails or is explicitly abandoned; recovery can repair only the exact consequence and cannot authorize another Agent turn. `conflict_result_retention_required` and `conflict_result_retention_failed` retain source ownership while exact Return repair is admissible. Source-level reliability and deterministic two-Host state validation are complete. Physical Mac ↔ Windows acceptance of movement, Return and apply is not complete; see [recorded results](platform/mac-windows-acceptance.md#recorded-results-for-200).

Native mature Agents are Host capabilities, separate from the Generic Managed Worker/provider path. `NativeAgentServiceV1` dispatches every invocation by capability ID to a registered `NativeCapabilityAdapterV1`; the only adapter registered in the shipped product is Codex: `agent.coding.codex`. The test-only `fake.longjob.v1` adapter exercises the same lifecycle. The control schema is `pastey-native-agent-control-v1`; task status is `pastey-native-agent-task-v1`; workspace-movement metadata/status uses `pastey-native-agent-workspace-movement-v1`; and the capability-neutral invocation uses `pastey-native-agent-invoke-v2`.

Native Codex capability state is exactly `available`, `incompatible`, or `unavailable`.

When it is not available, the capability fact's `unavailableReason` says which layer failed:

| Code | Meaning |
|---|---|
| `native_agent_not_installed` | Neither a Codex CLI nor a desktop app's bundled Codex was found. For the CLI: no `codex.exe` or `codex.cmd` on PATH. On Windows the search covers the process PATH plus the current user and system PATH read from the registry, and accepts app execution aliases. Elsewhere, `codex` is not found. For the desktop app: no valid `codex-cli/codex-package.json` (see [Layer 5](layers/layer-5-agent.md#native-codex-today)). |
| `native_agent_launch_failed` | Found, but could not be started. |
| `native_agent_probe_failed` | `codex --version` exited with an error. |
| `native_agent_probe_timeout` | No answer within 15 s. |
| `native_agent_incompatible` | `app-server` is unavailable. |
| `native_agent_unavailable` | Unknown, or an older peer. |

The local capability list also carries a `detail` naming the launcher and a bounded stderr excerpt, and a `backend`: `cli` or `desktop` (the Codex bundled with the desktop app), or absent for an explicitly configured executable. Neither is ever sent to a peer. On Windows, the launched Codex receives the merged PATH. A read-only diagnosis is in `scripts/diagnose-windows-codex.ps1`. `available` means this Host detected Codex and its bounded native app-server interface used by Pastey (initialize, thread/start, turn/start, lifecycle observation, interrupt, and shutdown) is usable. `incompatible` means Codex was detected but that interface is not usable by this build; `unavailable` means Codex cannot currently be invoked. The current-session peer capability observation carries `agent.coding.codex` plus exact supported Native Agent schemas. Direct remote Codex invoke requires only `pastey-native-agent-control-v1` and `pastey-native-agent-task-v1`; cross-Host workspace proposal, approval, and preparation require those two schemas plus `pastey-native-agent-workspace-movement-v1`. Codex advertises exactly those three. A generic invoke-v2 invocation requires the target capability to advertise `pastey-native-agent-control-v1` and `pastey-native-agent-invoke-v2`; Codex does not advertise invoke-v2. Each registered adapter contributes one `HostCapabilityFact` to that observation: `capabilityId`, `available`, effect `native_agent_compatibility`, the adapter's exact `supportedProtocols` when available, and `unavailableReason` `native_agent_incompatible` or `native_agent_unavailable` otherwise (`src-tauri/src/peer_capabilities.rs`). Compatibility is not Host selection or authority: installed/detected capability is not necessarily compatible, and compatible capability is not necessarily authorized. Capability compatibility does not authorize execution, Transfer, Review bypass, or session creation. A replaced session requires a fresh observation and cannot reuse the former binding. Execution on the executing Host additionally requires native executor admission (below).

The Native Agent control payloads are `NativeAgentInvokeV1` (Codex's original invocation), `NativeAgentInvokeV2` (capability-neutral invocation), `NativeAgentStatusV1`, `NativeAgentCancelV1`, `NativeAgentWorkspacePrepareV1`, `NativeAgentReconcileV1`, `NativeAgentReconciliationV1`, and `NativeAgentRetryResultReturnV1`. They reject unknown fields and bind the task identity, target/executing Host, capability, capability input, and movement correlation as appropriate. `NativeAgentInvokeV2.input`, task-status `output`, and reconciliation `taskOutput` are `OpaqueCapabilityPayloadV1`: capability-owned JSON bounded to 16 KiB that Core carries, persists, compares, and digests but never interprets. A v1 invoke is mapped onto the Codex adapter's input. Room Control's unsafe-field-name check applies to every event except exactly one opaque subtree, and only in the `native_agent` family: `/payload/input` of `native_agent.invoke` with `pastey-native-agent-invoke-v2`, `/payload/status/output` of `native_agent.status`, and `/payload/taskOutput` of `native_agent.reconciliation`, both with `pastey-native-agent-control-v1`. Their siblings are checked normally. Task `output` is untrusted capability-owned data: it reaches the renderer through every command that returns a task status, and neither Core nor the renderer may treat it as HTML, code, a command, a path, or a URL to act on. A recorded output cannot be replaced, and a status or reconciliation fact cannot change a task's capability. Native Agent Room Control uses protocol family `native_agent` and the following event kinds:

- `native_agent.invoke`
- `native_agent.status`
- `native_agent.cancel`
- `native_agent.workspace_prepare`
- `native_agent.reconcile`
- `native_agent.reconciliation`
- `native_agent.retry_result_return`
- `native_agent.device_identity`
- `native_agent.decision`
- `native_agent.session_query`
- `native_agent.session_response`

`NativeAgentStatusV1.status.nativeDecisions` carries at most 3 `NativeDecisionReviewV1` of a running task (`reviewRef`, `kind` `command_execution` | `file_change` | `unsupported`, `state` `pending` | `submitted` | `resolved`, `submitted`, `canAccept`, `canDecline`, `unavailableReason`, `content`, `contentDigest`, `requestedAt`, `expiresAt`); a status of any other state carries none. Each review's `content` (command, cwd, reason, network host/protocol, file changes with path, change, `moveTo` and diff) is native, untrusted display text bounded to 10 KiB, and is the only other subtree exempt from the field-name check, after `validate_status` checked its bound and that `contentDigest` is exactly its digest. `native_agent.decision` carries `NativeAgentDecisionV1` (`decisionId`, `taskId`, `targetHostRef`, `reviewRef`, `contentDigest`, `decision` `accept` | `decline`) and no content; its replay key is task, review and attempt. See Layer 5 for who may decide and when.

The renderer-visible task states are `queued`, `running`, `completed`, `failed`, `cancelled`, and `interrupted`. Workspace movement states are `review`, `awaiting_approval`, `transferring_to_agent`, `agent_running`, `returning_result`, `applying_result`, `completed`, `conflict_recovery_required`, `failed`, `cancelled`, and `interrupted`.

### Native Agent ownership and native executor admission

Every task has one immutable owner: Local, or the Bridge it was created on. The owner is durable as the envelope's Bridge correlation (or its movement's), and its absence means Local. No invocation, replay, workspace preparation, approval, or failed request from another principal can adopt, re-home, or erase it. Executor-side `native_agent.cancel` is accepted only from the owning Bridge; another Bridge's task, a Local task, and an unknown task receive the same refusal. `native_agent.workspace_prepare` is accepted only from the exact authenticated peer `HostSessionBinding` whose `HostRef` is the claimed source.

Native executor admission (`src-tauri/src/native_agent/admission.rs`) is the executor's separate decision that a principal may make this Host run a capability; it is unrelated to the managed Plan path's attempt-bound Host admission. The fixed policy admits `Local` immediately, admits `FullAccess(HostSessionBinding)` immediately, and requires Review for `Remote(HostSessionBinding)`. A peer is `FullAccess` only when the device identity verified in exactly that binding matches an active row of `full_access_grants` for this Bridge, peer HostRef and Host (`full_access::admits`); there is no other stored grant, per-capability ACL or per-peer memory. A capability that advertises no peer protocol is refused for both remote principals. A remote `native_agent.invoke`, and an approved movement's landed workspace, become `queued` with `native_agent_review_required`; no adapter `prepare` or `start` runs. For `native_agent.invoke` the executing Host checks, in order: the exact authenticated peer `HostSessionBinding`; target Host equal to itself; capability present and available; immutable ownership and replay; availability again; admission. A replay of an existing task is compared with the digest of the input Core admitted for it and never reaches an adapter; a replay whose admitted input is no longer known, for example after restart, is refused. A landed workspace reaches admission only after its tree is materialized and durably owned by its movement, which stays `transferring_to_agent` and keeps the landed tree and the requester's source owned while the executor Review is pending. Accept is one-shot and revalidates the exact binding, Bridge/Burn authority, Review expiry, ownership, availability, and, for a movement, that the landed workspace and held input still belong to that movement and task. Terminal admission codes, all with task state `failed`:

- `native_agent_admission_denied` — the Host user denied the Review.
- `native_agent_review_expired` — the Review expired.
- `native_agent_admission_revoked` — the exact session, Bridge, ownership, or capability ended before Accept could start it, or the Host restarted while it was pending.
- `native_agent_start_rejected` — accepted (or admitted under Full Access), but the capability refused the input or could not start it.

A pending Review is process-local and is never persisted: session loss ends it, Burn and peer departure remove it, and restart forgets it. A persisted movement task still marked `native_agent_review_required` becomes `failed` with `native_agent_admission_revoked` on restart, and its landed tree is cleaned through the existing terminal path. The renderer projection `NativeInvocationReviewV1` is exactly `taskId`, `bridgeId`, `peerHostRef`, `requestingPeerSessionId`, `capabilityDisplayName`, and `expiresAt` (Unix seconds)—never the invocation input—and is the same for direct invocations and received workspaces. Requester or local cancellation ends a pending executor Review as `cancelled`. Once started, an invocation keeps the existing lifecycle and is not re-admitted on route or session change.

Native task and movement identifiers are bounded to 256 bytes; opaque capability input and output are each bounded to 16 KiB; workspace input is bounded to 4 KiB; task text, a native stdout/stderr line, and retained stderr are each bounded at 16 KiB or 64 KiB as applicable. Native control RPC acknowledgement waits for initialize, thread/start, and turn/start are bounded, but a native turn has no Pastey-imposed execution or observation duration limit: silence alone leaves it `running`. Room Control and Transfer retain their own operation bounds. Workspace transfer metadata is additionally limited by the existing 10 GiB file-size bound. A remote invocation waits for Review at most 2 minutes, further bounded by the Bridge's expiry, and one Bridge may hold at most 8 unresolved Reviews. Source: `src-tauri/src/native_agent.rs`, `src-tauri/src/native_agent/admission.rs`, `src-tauri/src/native_agent/capability.rs`, `src-tauri/src/storage.rs`.

Registered Native Agent Tauri commands:

- `list_native_agent_capabilities`
- `start_native_codex_task`, `get_native_agent_task_status`, `cancel_native_agent_task`
- `start_remote_native_codex_task`, `start_remote_native_capability_task` (invoke-v2; no renderer wrapper yet), `cancel_remote_native_agent_task`
- `list_native_invocation_reviews`, `accept_native_invocation_review`, `deny_native_invocation_review`
- `propose_remote_native_codex_workspace_movement`, `approve_remote_native_codex_workspace_movement`, `get_native_agent_workspace_movement_status`
- `reveal_native_agent_conflict_result`, `discard_native_agent_conflict_result`
- `get_native_agent_recovery_projection`, `retry_native_agent_workspace_result_return`, `reconcile_remote_native_agent_task`, `stop_bridge_native_agent_task`

For an existing remote workspace, direct invocation uses the current authenticated Room Control session, waits for the executing Host's executor Review, and does not create a ManagedObject, Scratch, Worker, GST scan, or Transfer. When the selected local workspace must move to a remote Agent, proposal records the movement as awaiting requester movement approval; that approval covers prepare, encrypted outbound transfer, the request for the native task, encrypted return transfer, and unchanged-source apply. It is not execution authority on the executing Host: after landing, that Host's own executor Review must Accept before Codex is prepared or started. Multiple proposals may await Review, but one canonical workspace cannot have overlapping Native Agent execution or movement ownership. The Host-side check covers active movements, reconciliation-required or outcome-unknown movement/task facts, interrupted result apply, direct local execution, and the corresponding direct remote execution on its selected Host. Only terminal resolution or explicit cancellation releases ownership; durable task and movement envelopes preserve this guard across service restoration. Outbound/return metadata uses `NativeAgentWorkspaceTransferV1` with `outbound` or `return` phase and is carried by the existing transfer implementation, not a new transfer primitive. Loss of the final outbound Transfer acknowledgement, or a sender-local item-status write failure after receiver `/finish` success, is receipt-ambiguous because the receiver may already have materialized the workspace and, after its own Accept, started Codex; the source records reconciliation-required interruption, retains ownership, and performs no automatic resend or Agent rerun. Proven pre-transfer failure records ordinary interruption and cleans temporary package/item state. If the receiver already accepted `native_agent.workspace_prepare`, Pastey also makes a best-effort authenticated `native_agent.cancel` using a fresh current-session route; the prepared queued movement becomes terminal and rejects a late landing, while cleanup delivery failure remains an ordinary non-execution interruption. The original workspace is captured as a `RegularFileSet` baseline. Before applying the return, Pastey revalidates that baseline; a changed source first records the exact return identity, then creates and verifies a Host-private retained result under `native-agent-conflicts` plus its `native_agent_conflicts` SQLite record, and only then exposes `conflict_recovery_required`. Exact duplicate conflict Returns acknowledge that durable retained fact without reapplying or retaining again; conflicting Return correlation fails closed. The renderer receives no retained path: it may request Host-native reveal or exact discard. Discard first durably records `cancelled` with `conflict_result_discard_pending`, then removes only the verified private retained container and its database receipt, and finally records `conflict_result_discarded`; restart or repetition finishes that pending cleanup only.

`native_agent_envelopes` stores only Host-private outer task/movement correlation (including its authorizing Bridge when applicable), phase, task status, exact result snapshot/digest, and apply fact. Immutable correlation contains stable task/movement, target-Host, and task-digest facts only; the later private task-workspace landing is mutable recovery state. On startup, a task still awaiting Review becomes `failed` with `native_agent_admission_revoked` (it never started), and other queued/running or otherwise unproved Bridge-bound phases become `interrupted` with `native_agent_reconciliation_required`; a durably Completed received task with no snapshot seals the exact result from its retained app-owned task workspace and becomes Return-retryable without restoring or rerunning Codex. Unsafe or failed capture keeps Agent execution `Completed` and records an executor-side consequence failure. An authenticated reconciliation conveys that exact failure to the requester as `Interrupted / native_agent_result_snapshot_recovery_failed`; the user can explicitly abandon only movement authority to release the canonical source. An unbound local task that cannot be recovered through a Bridge becomes terminal `failed` with `native_agent_interrupted_on_restart` and its unreachable envelope is removed. Pastey never restores a Codex process, session, turn, or retry authority. The Bridge-scoped recovery projection returns only one unresolved safe lifecycle fact at a time; the existing card reloads it after each terminal resolution until every unresolved Bridge-bound fact has become reachable, and reconciliation always re-resolves through the current Layer 4 session. Remote uncertainty (`native_agent_reconciliation_required` and `native_agent_outcome_unknown`) exposes Reconcile and Stop; after either reconcile action, the renderer observes the durable task projection again without inferring a transition from silence. While a requester has `Completed` execution and `ReturningResult` without a consequence fact, the card periodically asks the selected Host for authenticated reconciliation; silence itself changes no state. An executor with a validated exact durable snapshot can prove its digest through fresh authenticated reconciliation. A requester interrupted before Return records that expected digest separately from any received result identity, restores `ReturningResult / result_return_retry_required`, and can ask the executor to resend only that snapshot. Return metadata and received content must match the expected digest; requester-side result identity is recorded only after receipt. The same retry state remains available across requester session loss and restart. `ReturningResult / result_return_retry_required` exposes Retry result Return for the already durable executor snapshot. `ConflictRecoveryRequired` exposes Reveal result and Discard result. An interrupted source apply or exact conflict-result retention failure exposes Retry result Return when the received result identity is durable; the requester reauthenticates the executing Host, which may resend only its matching Host-private snapshot. Exact conflict-retention recovery remains source-owning across requester restarts until retention succeeds or the user abandons it; both retry and Abandon recovery are available. A retried Return is compared by logical file-set content, so a fresh materialization of the same snapshot is accepted without requiring identical local file fingerprints. `WorkspaceApplyTransactionV1` journals the exact result identity before filesystem changes. After restart, Pastey restores the baseline if staging did not finish, completes a provably staged exact result, or recognizes a canonical result already committed. The apply is reported complete only after the canonical workspace matches the durable result; exact duplicate delivery is acknowledged without applying again. If a transaction cannot prove its paths, the journal remains and the workspace remains blocked for recovery. If a task is already Completed or Failed while a projected movement remains Interrupted, Stop or Abandon recovery durably cancels the movement with `native_agent_recovery_abandoned` while preserving the factual task outcome. Cancellation does not prove the native process exited: an unresolved remote task remains Host-private occupied until the native observer actually exits. Completed/failed/cancelled task facts, completed/applied or terminal movement phases, and running-over-queued ordering are monotonic under late status/reconciliation facts. A late native completion can therefore perform cleanup but cannot restore authority, move the workspace movement backward, return, or apply a result. Exact event-id replay remains rejected; repeated newly authenticated read-only reconcile queries are allowed, and status/reconciliation semantic replay identity covers the complete validated safe fact. Reused mutation identities must retain their immutable correlation or fail closed.

Transient Bridge route/session loss invalidates Bridge transport authority while preserving Host-local Codex sessions and any live native observer; a still-observed turn continues locally. Unobserved or uncertain task outcomes retain durable facts for fresh-session reconciliation. A completed task with an exact durable result snapshot stays `ReturningResult / result_return_retry_required` through session loss and restart; if restart precedes snapshot capture, Pastey seals it from the retained received workspace and enables only the exact Return retry. The retry remains bound to the current authenticated Bridge route and exact snapshot. Session loss does not add the Bridge to `revoked_bridges`. Burn remains destructive: it revokes Bridge-bound Native Agent authority and deletes that Bridge's envelopes, conflict receipts/artifacts, and app-owned transfer/result material. If an apply journal cannot prove the canonical workspace or hidden sibling trees, Burn leaves those trees untouched, logs incomplete cleanup, removes the Bridge-bound envelope, and allows burned-room startup finalization to continue. Direct local Native Agent tasks without Bridge correlation are untouched. Authenticated departure applies the local cutoff on the receiving Host. Late remote facts and Returns cannot recreate Burned authority. The native observer updates only an existing durable envelope, so a concurrent Burn deletion cannot be undone by its write. A local session cutoff cannot prove that a remote native process stopped across a partition.

After `turn/start` returns the exact thread/turn identity, Pastey observes until a native terminal fact, explicit user cancellation, shutdown, or loss of the native app-server/process channel; elapsed time and silence are not lifecycle facts. A lost acknowledgement or channel without an exact terminal outcome becomes `interrupted` with `native_agent_outcome_unknown`, never `failed` or `completed`, and stays workspace-occupied. Explicit cancellation or Burn may interrupt the Agent: an exact turn uses `turn/interrupt`; without an exact turn identity Pastey terminates and evicts only its own native app-server/session, then waits for the observer channel to exit. Local Bridge session loss only invalidates transport authority; Host-local observation and occupancy continue while the native observer is live, and uncertain outcomes remain available for a fresh Layer 4 session to reconcile. This does not assert that a remote Host's native process stopped across a partition.

Native sessions are Host-private. The Codex adapter keeps its app-server sessions per workspace, and only an exact requested `turn/completed` notification with `status: completed` and no error is terminal success. Cancellation wins a late completion; malformed, mismatched, failed, interrupted, or unknown outcomes are non-completion. Current-session resolution, replay checks, and rate limits remain the Room Control boundary; the Native Agent path does not expose provider credentials, native session IDs, raw workspace paths, or Agent reasoning.

### Native session discovery facts

Three local Tauri commands. They are not Room Control events, and no Bridge peer reaches them.

- `discover_native_sessions(query)`:
  - **Query:** `workspaces` (1 to 8 shareable folders), optional `provider` (`claude_code` or `codex`), optional `text`, optional `activeWithinSecs`. Unknown fields are refused.
  - **Result:** `observedAtMs`; per provider `historyReadable`, `liveSource` and `note`; `sessions`, newest activity first; `resolution` (`unique`, `ambiguous` or `none`), with `sessionId` only when unique.
- `inspect_native_session(request)`: `workspaces`, `provider` and `sessionId`. It answers only for a session whose recorded directory is one of the workspaces. Session ids must be a UUID (Claude Code) or `[A-Za-z0-9-]{1,64}` (Codex).

- `continue_native_session(request)`: `workspaces`, `provider`, `sessionId` and `prompt`. It accepts only `claude_code`, and only a session whose resume verdict is `safe`; otherwise it refuses with the reasons. Discovered Codex threads are refused. It returns the Native Agent task status of capability `agent.coding.claude_code`, which then follows `get_native_agent_task_status` and `cancel_native_agent_task`. That capability advertises no peer protocol, is not among the capability facts sent to peers, and Core refuses any Bridge invocation of it.

Each session carries:
- `provider`, `sessionId`, `workspace`, `title` (at most 120 characters);
- `origin` (`desktop`, `cli`, `pastey`, `sdk`, `other` or `unknown`) and `originRaw`;
- `createdAtMs`, `lastActivityAtMs`, `gitBranch`;
- `live`: `state` (`running`, `attached`, `not_running` or `unknown`), with `source`, `observedAtMs` and `detail`;
- `lastTurn`: `outcome` (`completed`, `interrupted`, `failed`, `in_progress`, `none` or `unreadable`), with `atMs` and `source`;
- `status` (`running`, `idle`, `completed`, `interrupted`, `failed` or `unknown`) and `statusBasis` (`fact`, `inference` or `unknown`);
- `resume`: `verdict` (`safe`, `unsafe` or `unknown`) and `reasons`.

A snapshot adds:
- `activity`: user turns, agent replies, tool calls, commands and failed commands, up to 20 changed files, errors;
- `historyComplete`;
- `recent`: up to 6 entries, `user` or `agent_claim`, each at most 600 characters;
- `workspace`: exists, Git branch, head, head time, uncommitted paths now, commits since the last activity;
- `evidence`: basis, statement, source.

Bounds:
- Claude transcripts are read up to 128 MiB each, and `claude agents --json` has 10 s.
- Codex reads are paged: 50 threads or turns per page, and up to 15 pages of 20 items, smaller when a page exceeds the 1 MiB line bound. Each request has 20 s.
- At most 200 sessions are returned per provider.

### Remote native session reading facts

A requester asks one exact current Bridge Host about its native sessions with `native_agent.session_query`. The Host answers with `native_agent.session_response` on the same session. Both payloads use schema `pastey-native-session-v1` and reject unknown fields. The replay keys are `native-session-query:<requestId>` and `native-session-response:<requestId>`.

**Query.** It has these fields:
- `requestId`, `targetHostRef`, and `operation` (`discover`, `inspect`, `workspaces`, which only lists the shared workspaces and looks at no session, or `open_folder`);
- optional `provider`, `text` (at most 200 characters, one line), `activeWithinSecs` and `scopeRef` (`scope:` followed by 16 hex characters);
- `sessionId`, which an inspection must name together with `provider`;
- `folder`, only and always with `open_folder`: the folder on that Host by its full path, one line, at most 1,024 characters.

**Authority.** The Host resolves the asker from the exact current peer session and the device identity proven in it (`verified_peer_device_for`). It answers only from that device's active **workspace shares**. The requester shows its identity before each query (`present_device_identity_over`), so a reconnect or a Host restart needs no other step.
- A share lives in the Host-local `session_read_grants` table (`src-tauri/src/session_read_access.rs`). Each row is one folder shared with one trusted device over one Bridge, with what it allows: `read_sessions` (and `share_excerpts`, only with it) and `agent_requests`. Rows made before workspace sharing keep `read_sessions = 1` and get `agent_requests = 0`.
- It binds the trusted device key, the Bridge, the peer HostRef, this Host and one canonical shareable workspace, and lasts 1 hour to 30 days.
- It is active only while unrevoked and unexpired, the device is still trusted, and the Bridge is neither burned nor removed. A moved workspace drops out. Revoking the device revokes its shares, so trusting it again revives none.
- At most 32 are active. Sharing a folder already shared with that device over that Bridge changes that share instead of adding one.
- Only the Host's local commands create, change, list or end shares (`share_bridge_workspace`, `update_workspace_share`, `stop_sharing_workspace`, `list_bridge_workspace_shares`, `list_session_read_grants`). `share_bridge_workspace` takes the Bridge and uses the device proven on it now (a fingerprint is required when more than one is); the device must already be trusted, and sharing never trusts it. Stopping a share also revokes that device's unattended grants for that folder on that Bridge. Trusting a device creates none. A share never satisfies an unattended grant.
- Burn deletes a Bridge's shares.
- **Full Access** (`src-tauri/src/full_access.rs`, tables `full_access_grants` and `full_access_folders`) lifts the switches: while the device proven in that session holds it, every share allows `read_sessions`, `share_excerpts` and `agent_requests` (the stored switches are unchanged), and the folders it added with `open_folder` count as shares with no expiry (`fullAccess: true`). `open_folder` validates the folder with the share rule (`validate_shareable_folder`) and adds it, at most 32 per grant; without Full Access, or for a refused folder, the answer is `folder_refused` with `detail`.
- The Host answers one query per Bridge at a time; another gets `busy`.

**Shared folder references.** A peer names a shared folder only as `scope:` followed by 16 hex characters: a BLAKE3 digest of the Bridge id and the canonical path, so two Bridges never see the same reference for one folder. The executing Host resolves every reference against that device's active shares in exactly that session (`resolve_agent_request`, called by Room Control before admission):
- a Codex run (`agent.coding.codex`, v1 invocation) whose `workspace` is a reference needs a share with `agent_requests`; a `workspace` that is a path keeps meaning that path, as before;
- a thread continuation (`agent.coding.codex_thread`) needs a share with both `agent_requests` and `read_sessions`, and refuses a path;
- anything else is refused with `native_agent_not_authorized` before admission; nothing is queued. An admitted request still waits for the Host's Review, unless the device has Full Access there, for runs and continuations alike.

**Answer.** It carries `requestId`, `sourceHostRef`, `outcome` (`ok`, `not_authorized`, `not_found`, `busy`, `unavailable` or `folder_refused`), and every folder shared with the device as `scopes` (`scopeRef`, folder `label`, `readSessions`, `shareExcerpts`, `agentRequests`, `expiresAt`, and `fullAccess` when only Full Access allows it; a scope from an older Host reads as readable without agent requests). `fullAccess: true` says the device holds the Host's Full Access now; `detail` says why a folder was refused, with any path replaced. Both are omitted when not set, so an older requester reads every other answer unchanged. Sessions come only from scopes with `readSessions`; `not_authorized` means none is readable (or the named one is not). It also carries either `discovery` or `snapshot`.
- **Minimal disclosure:**
  - no folder path: a workspace appears only as its folder name and `scopeRef`;
  - path-like text in statements and titles becomes `<path>`;
  - titles are at most 80 characters;
  - at most 50 sessions are sent, and `truncated` says when more matched, with the answer kept within 48 KiB;
  - changed files appear only relative to the workspace, at most 20, and files outside it are only counted (`changedElsewhere`);
  - `recent` holds at most 4 excerpts of 300 characters, only when a permission for that folder allows excerpts (`excerptsShared`), never whole transcripts.
- **Statuses** are the same as local discovery's: live state from a live source, history only for a stopped session.
- **Requester checks.** The requester accepts an answer only for a query it has outstanding (`register`, 60 s), on the same Bridge, from the Host it asked, once.

The requester's commands are `discover_remote_native_sessions(roomId, targetHostRef, provider?, text?, activeWithinSecs?, scopeRef?)`, `inspect_remote_native_session(roomId, targetHostRef, provider, sessionId)` and `open_remote_folder(roomId, targetHostRef, folder)`. Each waits up to 30 s.

### Native Agent source map

| Boundary | Primary source |
| --- | --- |
| Native capability, session, task, ownership, movement, baseline and conflict behavior | `src-tauri/src/native_agent.rs` |
| Native executor admission policy, pending executor Review, and renderer-safe Review projection | `src-tauri/src/native_agent/admission.rs` |
| Adapter seam, opaque capability payload, Codex adapter, and test-only `fake.longjob.v1` adapter | `src-tauri/src/native_agent/capability.rs`, `src-tauri/src/native_agent/capability/codex.rs`, `src-tauri/src/native_agent/capability/fake_longjob.rs` |
| Read-only native session discovery and inspection (Claude Code, Codex) | `src-tauri/src/native_agent/sessions.rs`, `src-tauri/src/native_agent/sessions/claude.rs`, `src-tauri/src/native_agent/sessions/codex.rs`; Codex history reader and holder scan in `src-tauri/src/native_agent/capability/codex.rs` |
| Local Claude Code session continuation (`agent.coding.claude_code`) | `src-tauri/src/native_agent/capability/claude_code.rs` |
| Remote session reading: permissions, wire types, minimal disclosure, Room Control kinds and requester wait | `src-tauri/src/session_read_access.rs`, `src-tauri/src/native_agent/sessions/remote.rs`, `src-tauri/src/room_control.rs` (`native_agent.session_query` / `session_response`), `src-tauri/src/bridge_lifecycle.rs` (`request_native_sessions`); UI in `src/features/workspace/BridgeWorkspaces.tsx` (Bridge details → Workspaces), `src/features/workspace/NativeSessionsCard.tsx`, `src/features/workspace/SessionReadingSettings.tsx` (Settings overview), `src/lib/workspaceShares.ts`, `src/lib/nativeSessions.ts` |
| Test-only deterministic requester/executor pair and external Agent-turn ledger | `src-tauri/src/native_agent_pair_harness.rs` |
| Registered commands and remote-session/movement dispatch | `src-tauri/src/commands.rs`, `src-tauri/src/main.rs` |
| Native Agent Room Control envelope, validation, replay/session handling | `src-tauri/src/room_control.rs` |
| Encrypted workspace package send, private landing, and movement registration | `src-tauri/src/transfer.rs`, `src-tauri/src/models.rs` |
| Host-owned Native Agent service lifetime | `src-tauri/src/host_runtime.rs` |
| Durable conflict table and retained-result validation | `src-tauri/src/storage.rs` |
| Renderer lifecycle/review controls | `src/features/workspace/AgentTaskLifecycle.tsx`, `src/features/workspace/BridgeWorkspace.tsx`; executor Agent-request Accept/Deny banner in `src/features/workspace/WorkspaceV2.tsx` |
| TypeScript status types and Tauri invocations | `src/lib/tauri.ts` |

## Host and managed authority source map

| Boundary | Primary source |
| --- | --- |
| `HostRuntime` and lifecycle | `src-tauri/src/host_runtime.rs` |
| `HostRef`, `PlanParticipantRef`, `HostSessionBinding` | `src-tauri/src/host_identity.rs` |
| Canonical current remote-Host session resolution and liveness proof | `src-tauri/src/bridge_lifecycle.rs`; opaque transfer consumption in `src-tauri/src/transfer.rs` |
| Managed Host admission (attempt-bound; not native executor admission) | `src-tauri/src/host_admission.rs` |
| Plan schema/protocol v2 | `src-tauri/src/bridge_plan_v2.rs` |
| Native-v2 product orchestration | `src-tauri/src/native_v2_orchestration.rs` |
| Native mature-Agent sessions and cross-device task envelope | `src-tauri/src/native_agent.rs` |
| Managed Worker coordination | `src-tauri/src/managed_worker_coordinator.rs` |
| Capability projection and generic semantic-ID syntax | `src-tauri/src/peer_capabilities.rs` |
| Fixed Host capability probes | `src-tauri/src/capability_probe.rs` |
| Capability-acquisition confirmation | `src-tauri/src/capability_acquisition_confirmation.rs`, `src-tauri/src/commands.rs`, `src/lib/tauri.ts`, `src/lib/types.ts` |
| Host-local managed runtime configuration | `src-tauri/src/managed_runtime_config.rs`, `src-tauri/src/capability_probe.rs` |
| Managed objects | `src-tauri/src/managed_objects.rs` |
| Safe physical identity | `src-tauri/src/safe_file_identity.rs` |
| Effect contracts and state | `src-tauri/src/effect_authority.rs` |
| Resource backend | `src-tauri/src/managed_resources.rs` |
| Process world controller | `src-tauri/src/execution_world.rs` |
| Platform execution backend seam | `src-tauri/src/execution_backend.rs` |
| Windows Codex-derived process backend and setup | Pastey adapter `src-tauri/src/windows_codex_backend.rs`; pinned mechanics `src-tauri/crates/windows-codex-sandbox/`; setup command `--pastey-setup-windows-codex-sandbox-v1`; verifier command `--pastey-verify-windows-codex-sandbox-v1`; provenance `UPSTREAM.md`; local divergence `PATCHES.md` |
| Network broker | `src-tauri/src/network_broker.rs` |
| Core claim/result finalizer | `src-tauri/src/managed_execution.rs` |
| Worker Harness/provider | `src-tauri/src/worker_harness.rs`, `worker_provider.rs` |
| Provider configuration | `src-tauri/src/worker_provider_config.rs` |
| Bridge Device Check / Managed E2E self-check | `src-tauri/src/commands.rs`, `src-tauri/src/diagnostics.rs`, `src/lib/tauri.ts`, `src/lib/types.ts` |
| Bridge NodeList projection | `src-tauri/src/diagnostics.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/host_runtime.rs`, `src/lib/tauri.ts`, `src/lib/types.ts` |

The fixed-probe and acquisition-ID domains are intentionally distinct:

```text
acquisition intent:
  runtime.java → valid semantic ID

fixed Host probe:
  runtime.java → rejected until a fixed-probe vocabulary entry and implementation exist
```

Capability is not authority, probe availability is not executable binding, and acquisition confirmation is neither installation nor execution authority.

## Provider configuration facts

The Host service stores non-secret provider id, generation, config digest, HTTPS base URL, model, timeout, output-token limit, health, and timestamps in SQLite. The API key is stored separately as authenticated ciphertext under the existing Host master key. Exact generations are immutable run bindings; update increments generation, delete revokes active bindings, and stale references fail closed.

Accepted production endpoints must use HTTPS and valid bounded model/config values. Provider health has `unknown`, `healthy`, and `unhealthy` states. A healthy observation requires bounded compatible response shape plus the exact configured returned model; it performs no Worker task effect and grants no execution authority. Local Tauri commands `get_managed_worker_provider_settings`, `create_managed_worker_provider`, `update_managed_worker_provider`, `delete_managed_worker_provider`, `select_managed_worker_provider`, and `check_managed_worker_provider_health` return only the renderer-safe settings snapshot. Create/update receive a credential only for that command; no list/status/read operation or command returns it. Environment-variable provider loading remains limited to an ignored opt-in development smoke test.

Minimum Real Provider Conformance is implemented for the exact generic OpenAI-compatible request profile: strict response-model identity, bounded failure classification, cancellation-aware transport, and one-call streaming assembly. A supported external provider/model still means that exact configuration and model has recorded Pastey conformance acceptance; no compatible provider, health observation, or configuration alone claims a Managed E2E `PASS`.

## Room Control message lifecycle and backpressure

Source: `src-tauri/src/room_control.rs`. Every received event is first authenticated (sender transport key, decryption, `validate_control_event`: schema, session refs, expiry, size, allowed kind, unsafe fields). Then:

**Admission, before anything acts on it** (`admit_received_event`). The event must be new to this Bridge's replay window (event, envelope and request ids, 256 each) and fit its class's rate budget. Only an admitted event records its replay ids. A refusal changes nothing but the diagnostic counters, so the sender may send a fresh event. For capability and Bridge Plan events, a refusal after admission (invalid content, authority, session) releases the replay ids again (`AdmittedEventV1`), so a refused event leaves no trace, as when these checks ran last. Native Agent events keep their ids once admitted (a task id is answered idempotently by Core).

| Class | Kinds | Budget per minute / per 2 s | Kept after handling |
|---|---|---|---|
| `capability_query` | `peer_capability.query` | 60 / 12 | No: answered (off the request path, on the blocking pool) |
| `capability_answer` | `peer_capability.response` | 120 / 24 (answers to this Host's own checks; twice the query budget absorbs arrival jitter) | No: recorded as the peer's capability fact |
| `native_agent` | `native_agent.*` | 90 / 20 | No: Core's task, Review and session state is the record |
| `bridge_plan` | `bridge_plan.*`, including v2 | 30 / 8 | v1 only, as display history |
| `membership` | `bridge_membership.*` | 30 / 8 | No |

Developer Terminal frames keep their own separate flow control and replay set.

**Display history.** Only received Bridge Plan v1 events are kept, for Find File's progress (`list_received_room_control_events`): at most 64 per Bridge and 30 minutes each, oldest dropped first. It is observation only (the Bridge Plan stores hold the state of record), so it never refuses an event; there is no `inbox_full` refusal any more. Capability facts, Native Agent events and Bridge Plan v2 events are never kept here. Room server stop (Repair, restart) and Burn clear it with the replay and rate state.

**Pending interactions** keep their own lifecycle outside Room Control. A native capability Review is `pending` until Accept, Deny, its 2-minute expiry (bounded by the Bridge's), or the end of its authority; one Bridge may hold 8, and a ninth is refused with `native_agent_review_capacity` (429) before anything is queued, never by dropping a waiting one. Accept and Deny each consume the Review once. Accept re-checks the session, Bridge, ownership and capability and, for an invocation that came through a shared folder, that the share still allows it (`share_requirement_holds`: unrevoked, unexpired, device still trusted with the same key, Bridge not burned, folder unmoved, Agent requests and, for a continuation, Sessions still on).

**Refusal codes and what the sender may conclude** (`DELIVERY_REFUSALS`, `delivery_failure`):

| Sender's reading | Codes | Meaning |
|---|---|---|
| `rate_limited` | `rate_limited`, `terminal_rate_limited` | Refused before acting. Safe to send a fresh event later. |
| `review_capacity` | `native_agent_review_capacity` | Nothing queued; no waiting Review dropped. |
| `refused` | `native_agent_not_authorized`, `native_agent_incompatible`, `host_mismatch`, `invalid_native_agent` | Refused before admitting anything. |
| `replayed`, `expired` | `event_replayed`, `event_expired` | Already received, or too late. |
| `inbox_full` | `inbox_full` (older Pastey only) | An older receiver refused after acting: not proof of anything. |
| `rejected` | every other code | Refused; whether it acted first is not known. |
| `transport` | timeouts, unreadable answers | Unknown whether it arrived. |
| `not_sent` | preparation failed here | Never sent. |

`not_sent`, `rate_limited`, `review_capacity` and `refused` prove the receiver did nothing (`proves_nothing_happened`). A Native Agent invocation refused that way ends on the requester as `failed` / `native_agent_not_delivered` with the receiver's reason; every other failure keeps the existing `native_agent_reconciliation_required` interruption, and nothing is ever re-run automatically.

**Retrying.** `send_room_control_event_with_backpressure` builds a fresh event for each attempt and retries only on `rate_limited`, within a stated patience (capability query and answer 5 s, session query 5 s, its answer 10 s, identity 4 s, invocation 6 s, a received-workspace status 30 s). The native status loop offers one unchanged status again while it is rate-limited for up to 120 s without using its 5 resends, and a final status is resent until settled (delivered or resends used up), not just once. Nothing retries an unknown outcome.

**Diagnostics** (`get_room_control_diagnostics`, Device Details → Advanced → Control channel): per class accepted and refused counts by reason, use of the last minute against the budget, current backpressure (since when, how many refused), history depth and drops, replay window, outbound failures by reading, the last refusal each way, and the Bridge's Reviews by outcome. Counts, classes, reasons and times only. Logs add one line when a class enters backpressure (then every 100th refusal) and one when it ends, with the Bridge id's first 8 characters.

## Bridge authentication facts

Protocol `pastey-bridge-auth-v1` (`src-tauri/src/bridge_auth.rs`). Discovery messages travel as JSON over UDP port 48392; `/join` and the authenticated room routes over the room server's HTTP port.

| Message / route | Carries | Authenticated by |
| --- | --- | --- |
| `pair_query` | `request_id`, SPAKE2 message A | nothing (contains nothing checkable offline) |
| `pair_offer` | SPAKE2 message B, Bridge id, port, expiry, transport key, device name, HostRef, Ed25519 device key, `confirm` | key confirmation under the SPAKE2 key over the whole offer |
| `/join` with `auth.mode = "pair"` | joiner transport key, port, name, HostRef, device key, signature, `confirm` | SPAKE2 key confirmation plus the joiner's device signature over the offer and join transcript |
| `/join` reply with `auth` | Host signature and `confirm` | the Host's device key confirmed in the offer, plus key confirmation |
| `reconnect_query` | Bridge id, requester HostRef, 32-byte nonce | nothing; answered only for the pinned HostRef |
| `reconnect_offer` | port, expiry, transport key, HostRef, single-use 32-byte Host nonce, signature | the pinned device key over both nonces and the offer |
| `/join` with `auth.mode = "reconnect"` | both nonces, signature | the pinned device key over both nonces and both transport keys |
| `diagnostics/ping`, `diagnostics/benchmark/{raw,pipeline}`, `transfers/{id}/finish`, `transfers/{id}/cancel` | headers `x-pastey-sender-key` and `x-pastey-request-auth: v1.<unix seconds>.<32 hex nonce>.<mac>` | keyed BLAKE3 MAC under HKDF of the X25519 agreement between both current room-server keys, over sender and receiver keys, Bridge id, route, time, nonce and body hash; the sender key must be the Bridge's single connected peer |
| `items`, `transfers/start` | `sender_public_key` and an X25519-wrapped key | the sender key must be the Bridge's single connected peer |
| `control-events` | encrypted Room Control envelope | unchanged (see Room Control) |

Bounds: a pairing code answers at most 20 pairing queries and expires after 15 minutes; one success closes it. A joiner evaluates at most 16 offers per attempt. A pending pairing lasts 120 s; a reconnect nonce 60 s, at most 16 per Bridge. Request timestamps may differ by 120 s; each room server remembers at most 4,096 request nonces. Nearby join prompts: one pending per source address, four in total.

Transfer integrity:
- **Chunk binding:** each chunk is ChaCha20-Poly1305 under the transfer's session key, with associated data `pastey-transfer-chunk-v2\0` ‖ len(transfer id) ‖ transfer id ‖ chunk index ‖ total chunks ‖ plaintext size (big-endian u64 each) ‖ final flag (1 byte). The format is defined in `src-tauri/src/chunk_frame.rs`.
- **Final digest:** the signed `finish` request carries `plaintext_blake3`, the BLAKE3 of the whole plaintext; the receiver re-reads the assembled file and finalizes only on a match.
- **Unauthenticated chunks:** a chunk request that fails before or during decryption is refused alone and never ends the transfer. Only an authenticated inconsistency or a local write failure fails it.

Fresh trust: pairing a device key other than the Bridge's current pin runs `establish_fresh_peer_trust` (`src-tauri/src/commands.rs`) before pinning. It removes:
- Bridge-scoped runtime authority (`purge_bridge_runtime_authority`, after which the Bridge is re-admitted for new work);
- durable Native Agent Bridge state (`purge_durable_bridge_state`);
- `authorized_search_folders` rows for the Bridge.

A code entered under New Bridge, nearby or overlay join is refused if it names a Bridge this device already has. `repair_bridge` (Bridge id and code) pairs exactly that Bridge again, trying the remembered address first and then the LAN.

Known devices: `known_bridge_devices` holds each remembered device's key, HostRef, label, first and last pairing time, and revocation time. A nearby `join_request` may carry `known`, a `KnownConnectProof` with these fields:
- `devicePublicKey`, `targetHostRef`, `issuedAt` and a 32-byte `nonce`;
- an Ed25519 signature over request id, requester HostRef, target HostRef, key, time and nonce, with purpose `known-connect-request` and 120 s skew.

The receiver verifies it against its remembered key and the requester's `device_id`-derived HostRef, claims the request id once, pins a new Bridge, and replies with `known_offer`, a `reconnect_offer` for that nonce, and `bridge_label`, the Bridge's display code with no pairing window open. `request_nearby_join` returns a `NearbyJoinOutcome`: `connected` with a room, or `code_required` with a request id and device name. Commands: `list_known_devices`, `forget_known_device` (by fingerprint). Events: `pastey://known-device-connect`.

Storage: `bridge_peer_pins` (Bridge id → peer HostRef and device key) and `bridge_pairing_windows` (open time, expiry, answered count, close time). Both are deleted at the Burn authority cutoff; an authenticated departure deletes the pin. `RoomInfo.pairing_state` is `paired`, `awaiting_peer` or `pairing_required`. Commands: `open_bridge_pairing` (fresh code for an existing Bridge), `repair_bridge` (pair an existing Bridge with the other device's code), `request_nearby_join` (returns a pending request), `complete_nearby_join` (request id and code).

## Internet carrier facts

Design and contract: [Connectivity](layers/layer-4-connectivity.md). Source: `src-tauri/src/connectivity.rs`, `connectivity/iroh_carrier.rs`.

| Item | Value |
| --- | --- |
| Setting | `config.internet = { enabled, relayUrl }` (off and `null` by default); commands `get_internet_connectivity`, `set_internet_connectivity(enabled, relayUrl)`, `get_bridge_internet_status(roomId)` → `{ carrierRunning, peerReachableByCarrier, path: "direct" \| "relayed" \| "unknown" \| null }` |
| Carrier | `iroh` 1.3 (features `tls-ring`, `portmapper`, `fast-apple-datapath`), ALPN `pastey/bridge-tunnel/1`; default relays n0's public relays with n0 DNS address publishing, or `RelayMode::Custom` with the user's `https://` relay and no publishing |
| Carrier key | Ed25519, one per installation and carrier, in `carrier_identity`, wrapped by the installation master key; separate from the device identity key |
| Card | `CarrierCardV1 { schemaVersion: "pastey-carrier-card-v1", carrier, endpointId, relayUrl?, directAddrs[] }`, unknown fields refused. `carrier` must be known (`iroh`); `endpointId` 1–128 of `[A-Za-z0-9-]`; at most 8 `directAddrs`, each a socket address; `relayUrl` `https://`, at most 512 characters |
| Card event | Room Control `bridge_membership.carrier_card`, payload `{ schemaVersion: "pastey-bridge-membership-v1", card }`, Membership rate class (30/min, burst 8). Pinned for the session's authenticated peer HostRef only when the Bridge's `bridge_peer_pins` row names it; otherwise refused with `carrier_card_refused`. Announced to connected peers when the card changed, at most every 30 s per peer session |
| Pin storage | `bridge_carrier_pins (room_id, peer_host_ref, carrier, endpoint_id, relay_url, direct_addrs, updated_at)`; read only joined to a live `bridge_peer_pins` row for the same HostRef on an unburned Bridge; deleted at the Burn cutoff and on departure |
| Tunnel stream | First a header: big-endian u16 length, then JSON `{ protocol: "pastey-bridge-tunnel-v1", kind: "room" \| "reconnect", bridgeId }`, at most 1,024 bytes, within 10 s. `room`: raw bytes to that Bridge's room server. `reconnect`: one big-endian u32-length JSON `reconnect_query` (at most 16 KiB) whose Bridge id and requester HostRef must match the header and the pin; the answer is one `reconnect_offer`, or an empty message for none |
| Timeouts | Carrier connect 12 s; reconnect answer 12 s; tunnel liveness probe 5 s, 4 failures (LAN 1.8 s, 3) |
| Endpoint | The peer endpoint of a tunnelled session is `127.0.0.1:<forwarder port>`, one forwarder per (Bridge, peer carrier key), stable across path changes and carrier reconnects |

## Full Access facts

| Fact | Value |
| --- | --- |
| Source | `src-tauri/src/full_access.rs`; admission in `native_agent/admission.rs` (`FullAccess` principal) |
| Tables | `full_access_grants` (grant id, Bridge, peer HostRef, local HostRef, device fingerprint and base64 public key, granted/revoked time); `full_access_folders` (grant id, canonical folder, added time) |
| Active when | `FULL_ACCESS_IS_ACTIVE` in `storage.rs`: unrevoked; `trusted_devices` row with the same fingerprint and key, unrevoked; `bridge_peer_pins` row for the Bridge with the same peer HostRef and key; Bridge row live and not tombstoned |
| Created by | `grant_full_access({ roomId, understood: true })`, local only; uses the Bridge's pin, trusts that key, keeps one active grant per Bridge |
| Ended by | `revoke_full_access`, `revoke_trusted_device`, `forget_known_device`, pairing the Bridge with another key (`establish_fresh_peer_trust`), Burn (cutoff, then `delete_bridge_full_access`) |
| On revoke | `end_full_access_operations`: closes the Bridge's hosted terminals (`host_closed`); `stop_full_access_tasks` asks every task the grant admitted to stop, recorded as `native_agent_full_access_revoked` (or `native_agent_cancel_delivery_uncertain`) with state `cancelled` |
| Process rights | `get_bridge_full_access` reports `processRights`: `standard`, `administrator` (elevated Windows token or root) or `unknown`; everything Full Access starts inherits them |
| Other local commands | `get_bridge_full_access`, `list_full_access`, `remove_full_access_folder` |
| Requester command | `open_remote_folder(roomId, targetHostRef, folder)` |
| Folders | Same rule as a share (`validate_shareable_folder`); at most 32 per grant; forgotten on revoke |
| Retired | `unattended_grants` and `unattended_grant_uses` are dropped at startup (`retire_unattended_grants`); nothing is converted |

## Developer Terminal protocol and bounds

Protocol family/version: `developer_terminal` / `pastey-developer-terminal-v0`.

Message kinds:

- `developer_terminal.open_request`
- `developer_terminal.open_accepted`
- `developer_terminal.open_denied`
- `developer_terminal.input`
- `developer_terminal.output`
- `developer_terminal.resize`
- `developer_terminal.exit`
- `developer_terminal.close`

Current limits:

- 8 KiB maximum input/output frame;
- 64-frame bounded Host PTY output channel;
- 512 KiB bounded controller display buffer;
- 3,000 receiver events per minute and 256 events per two-second burst;
- 64 KiB ordered controller input queue;
- 5,000-line xterm scrollback;
- 30-minute UI and active-session lifetime;
- 2-minute admission-request lifetime.

A pending `open_request` from a controller whose device, proven in exactly that session, holds the target Host's Full Access is admitted by the target itself (`admit_open_with_full_access`, starting at 100×30 until the controller resizes it). Its Host session is marked `fullAccess` in the target's workspace projection. The target's user can end it (`close_hosted_developer_terminal`), and revoking Full Access or the device ends every such session on that Bridge; the controller receives `developer_terminal.exit` with `reasonCode: host_closed`.

The frontend uses `@xterm/xterm` and `@xterm/addon-fit`. Host shell selection is Host-owned: an allowed `$SHELL` or safe fallback on Unix, and PowerShell through ConPTY on Windows. Terminal content and absolute paths are excluded from ordinary Pastey logs/history.

## Validation map

| Boundary | Focused validation |
| --- | --- |
| Natural proposals | `scripts/run-natural-v1-tests.mjs`, `scripts/run-natural-v2-tests.mjs`, Rust `natural_v2` tests |
| Plan lifecycle and native-v2 orchestration | Rust `host_identity`, `host_runtime`, `host_admission`, `bridge_plan`, `bridge_plan_v2`, `native_v2_orchestration`, and `managed_worker_coordinator` tests |
| Worker/provider/runtime configuration | Rust `worker_harness`, `worker_provider`, `worker_provider_config`, `managed_runtime_config`, and `managed_worker_coordinator` tests |
| NodeList/capability/confirmation boundaries | Rust `diagnostics`, `commands`, `peer_capabilities`, `capability_probe`, and `capability_acquisition_confirmation` tests; frontend integration tests |
| Effects/results | Rust `effect_authority`, `managed_resources`, `execution_world`, `network_broker`, and `managed_execution` tests; opt-in native Windows `windows_execution_world` integration test |
| Layer 4 and transfer | `scripts/run-layer4-validation-matrix.mjs`, `scripts/run-transfer-planner-tests.mjs`, Rust transport/protocol tests |
| Bridge pairing, reconnect and room-request authentication | Rust `bridge_auth`, `transfer::join_authentication`, `discovery`, `bridge_lifecycle`, `storage` and `commands` tests |
| Developer Terminal | Rust terminal/HostRuntime tests plus native physical platform checks |
| Native Agent task/session/movement and conflict recovery | Rust `native_agent`, `commands`, `room_control`, `transfer`, `storage`, and `host_runtime` tests, including the test-only two-Host pair harness; renderer types/lifecycle in `src/lib/tauri.ts` and `src/features/workspace/AgentTaskLifecycle.tsx` |
| Native capability neutrality, ownership, and executor admission | Rust `native_agent::tests::pair_harness` (`fake.longjob.v1` capability pair; direct and received-workspace admission scenarios); `peer_capabilities` and `room_control` opaque-boundary tests; `npm run test:frontend-integration` for the Review projection and banner |

The full contributor and physical validation procedure is in [development](development.md).
