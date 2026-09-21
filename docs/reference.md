# Pastey reference

This document owns concrete identifiers, bounds, configuration facts, and source pointers. Source types and validators remain authoritative; architecture is in [architecture](architecture.md), [Layer 5](layers/layer-5-agent.md), and [Windows managed execution](platform/windows-managed-execution.md).

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

Native mature Agents are Host capabilities, separate from the Generic Managed Worker/provider path. The current concrete capability is Codex: `agent.coding.codex`. Its control schema is `pastey-native-agent-control-v1`; task status is `pastey-native-agent-task-v1`; and workspace-movement metadata/status uses `pastey-native-agent-workspace-movement-v1`.

Native Codex capability state is exactly `available`, `incompatible`, or `unavailable`. `available` means this Host detected Codex and its bounded native app-server interface used by Pastey (initialize, thread/start, turn/start, lifecycle observation, interrupt, and shutdown) is usable. `incompatible` means Codex was detected but that interface is not usable by this build; `unavailable` means Codex cannot currently be invoked. The current-session peer capability observation carries `agent.coding.codex` plus exact supported Native Agent schemas. Direct remote invoke requires only `pastey-native-agent-control-v1` and `pastey-native-agent-task-v1`; cross-Host workspace proposal, approval, and preparation require those two schemas plus `pastey-native-agent-workspace-movement-v1`. Compatibility is not Host selection or authority: `detected != compatible`, `compatible != authorized`, and capability compatibility does not authorize execution, Transfer, Review bypass, or session creation. A replaced session requires a fresh observation and cannot reuse the former binding.

The Native Agent control payloads are `NativeAgentInvokeV1`, `NativeAgentStatusV1`, `NativeAgentCancelV1`, `NativeAgentWorkspacePrepareV1`, `NativeAgentReconcileV1`, and `NativeAgentReconciliationV1`. They reject unknown fields and bind the task identity, target/executing Host, capability, workspace/task inputs, and movement correlation as appropriate. Native Agent Room Control uses protocol family `native_agent` and the following event kinds:

- `native_agent.invoke`
- `native_agent.status`
- `native_agent.cancel`
- `native_agent.workspace_prepare`
- `native_agent.reconcile`
- `native_agent.reconciliation`

The renderer-visible task states are `queued`, `running`, `completed`, `failed`, `cancelled`, and `interrupted`. Workspace movement states are `review`, `awaiting_approval`, `transferring_to_agent`, `agent_running`, `returning_result`, `applying_result`, `completed`, `conflict_recovery_required`, `failed`, `cancelled`, and `interrupted`.

Native task and movement identifiers are bounded to 256 bytes; workspace input is bounded to 4 KiB; task text, a native stdout/stderr line, and retained stderr are each bounded at 16 KiB or 64 KiB as applicable. Native control RPC acknowledgement waits for initialize, thread/start, and turn/start are bounded, but a native turn has no Pastey-imposed execution or observation duration limit: silence alone leaves it `running`. Room Control and Transfer retain their own operation bounds. Workspace transfer metadata is additionally limited by the existing 10 GiB file-size bound. Source: `src-tauri/src/native_agent.rs`, `src-tauri/src/storage.rs`.

Registered Native Agent Tauri commands:

- `list_native_agent_capabilities`
- `start_native_codex_task`, `get_native_agent_task_status`, `cancel_native_agent_task`
- `start_remote_native_codex_task`, `cancel_remote_native_agent_task`
- `propose_remote_native_codex_workspace_movement`, `approve_remote_native_codex_workspace_movement`, `get_native_agent_workspace_movement_status`
- `reveal_native_agent_conflict_result`, `discard_native_agent_conflict_result`
- `retry_native_agent_workspace_result_return`, `reconcile_remote_native_agent_task`

For an existing remote workspace, direct invocation uses the current authenticated Room Control session and does not create a ManagedObject, Scratch, Worker, GST scan, or Transfer. When the selected local workspace must move to a remote Agent, proposal records the movement as requiring review; one approval covers prepare, encrypted outbound transfer, native task, encrypted return transfer, and unchanged-source apply. Multiple proposals may await Review, but approval fails closed when another nonterminal approved movement already owns the same canonical source workspace; the durable movement envelope supplies that guard across service restoration. Outbound/return metadata uses `NativeAgentWorkspaceTransferV1` with `outbound` or `return` phase and is carried by the existing transfer implementation, not a new transfer primitive. The original workspace is captured as a `RegularFileSet` baseline. Before applying the return, Pastey revalidates that baseline; a changed source first records the exact return identity, then creates and verifies a Host-private retained result under `native-agent-conflicts` plus its `native_agent_conflicts` SQLite record, and only then exposes `conflict_recovery_required`. Exact duplicate conflict Returns acknowledge that durable retained fact without reapplying or retaining again; conflicting Return correlation fails closed. The renderer receives no retained path: it may request Host-native reveal or exact discard. Discard first durably records `cancelled` with `conflict_result_discard_pending`, then removes only the verified private retained container and its database receipt, and finally records `conflict_result_discarded`; restart or repetition finishes that pending cleanup only. Completed, failed, cancelled, and interrupted movement history remains visible but does not block a later task; unresolved conflict recovery does.

`native_agent_envelopes` stores only Host-private outer task/movement correlation, phase, task status, exact result snapshot/digest, and apply fact. Immutable correlation contains stable task/movement, target-Host, and task-digest facts only; the later private task-workspace landing is mutable recovery state. On startup, queued/running or otherwise unproved phases become `interrupted` with `native_agent_reconciliation_required`; Pastey never restores a Codex process, session, turn, or retry authority. Cancellation immediately records both task and matching non-applied movement as `cancelled`, revoking Pastey execution/result authority, but does not prove the native process exited: its workspace remains Host-private occupied until the native observer actually exits. A late native completion can therefore perform cleanup but cannot restore `completed`, move the workspace movement to `returning_result`, or return/apply a result. A durable result snapshot keeps `returning_result` retryable through the existing encrypted Transfer. A durable completed apply is idempotent and an exact duplicate Return Transfer is acknowledged without applying again; conflicting result correlation fails closed. Reused identities must retain their immutable correlation or fail closed. Reconciliation re-resolves the durable remote `HostRef` through the existing Layer 4 current-session binding; the old session binding is never accepted again, and facts disclose no private paths, credentials, session IDs, reasoning, or Agent process/tool state.

After `turn/start` returns the exact thread/turn identity, Pastey observes until a native terminal fact, explicit cancellation, shutdown, or loss of the native app-server/process channel; elapsed time and silence are not lifecycle facts. A lost acknowledgement or channel without an exact terminal outcome becomes `interrupted` with `native_agent_outcome_unknown`, never `failed` or `completed`, and stays workspace-occupied. It is terminated only by explicit user cancellation or Host shutdown: an exact turn uses `turn/interrupt`; without an exact turn identity Pastey terminates only its own native app-server/session and waits for the observer channel to exit. Requester/control disconnection similarly does not cancel the executing Host; a fresh Layer 4 current session may reconcile its durable fact while the old binding remains invalid.

Native sessions are Host-private. `NativeAgentServiceV1` keeps Codex sessions per workspace, and only an exact requested `turn/completed` notification with `status: completed` and no error is terminal success. Cancellation wins a late completion; malformed, mismatched, failed, interrupted, or unknown outcomes are non-completion. Current-session resolution, replay checks, and rate limits remain the Room Control boundary; the Native Agent path does not expose provider credentials, native session IDs, raw workspace paths, or Agent reasoning.

### Native Agent source map

| Boundary | Primary source |
| --- | --- |
| Native capability, session, task, movement, baseline and conflict behavior | `src-tauri/src/native_agent.rs` |
| Registered commands and remote-session/movement dispatch | `src-tauri/src/commands.rs`, `src-tauri/src/main.rs` |
| Native Agent Room Control envelope, validation, replay/session handling | `src-tauri/src/room_control.rs` |
| Encrypted workspace package send, private landing, and movement registration | `src-tauri/src/transfer.rs`, `src-tauri/src/models.rs` |
| Host-owned Native Agent service lifetime | `src-tauri/src/host_runtime.rs` |
| Durable conflict table and retained-result validation | `src-tauri/src/storage.rs` |
| Renderer lifecycle/review controls | `src/features/workspace/AgentTaskLifecycle.tsx`, `src/features/workspace/BridgeWorkspace.tsx` |
| TypeScript status types and Tauri invocations | `src/lib/tauri.ts` |

## Host and managed authority source map

| Boundary | Primary source |
| --- | --- |
| `HostRuntime` and lifecycle | `src-tauri/src/host_runtime.rs` |
| `HostRef`, `PlanParticipantRef`, `HostSessionBinding` | `src-tauri/src/host_identity.rs` |
| Canonical current remote-Host session resolution and liveness proof | `src-tauri/src/bridge_lifecycle.rs`; opaque transfer consumption in `src-tauri/src/transfer.rs` |
| Host admission | `src-tauri/src/host_admission.rs` |
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
| Developer Terminal | Rust terminal/HostRuntime tests plus native physical platform checks |
| Native Agent task/session/movement and conflict recovery | Rust `native_agent`, `commands`, `room_control`, `transfer`, `storage`, and `host_runtime` tests; renderer types/lifecycle in `src/lib/tauri.ts` and `src/features/workspace/AgentTaskLifecycle.tsx` |

The full contributor and physical validation procedure is in [development](development.md).
