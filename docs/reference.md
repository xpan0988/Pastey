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
| Fixed Host system-probe request | `capabilityIds`: at most 12 deduplicated IDs from the global semantic vocabulary; the receiving Host alone maps an ID to a fixed local probe. No path, command, args, shell, or acquisition fields. |
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

Only the Natural-v2 candidate command currently has a TypeScript wrapper in `src/lib/tauri.ts`. The remaining commands are backend seams for the later product UI.

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

## Host and managed authority source map

| Boundary | Primary source |
| --- | --- |
| `HostRuntime` and lifecycle | `src-tauri/src/host_runtime.rs` |
| `HostRef`, `PlanParticipantRef`, `HostSessionBinding` | `src-tauri/src/host_identity.rs` |
| Canonical current remote-Host session resolution and liveness proof | `src-tauri/src/bridge_lifecycle.rs`; opaque transfer consumption in `src-tauri/src/transfer.rs` |
| Host admission | `src-tauri/src/host_admission.rs` |
| Plan schema/protocol v2 | `src-tauri/src/bridge_plan_v2.rs` |
| Native-v2 product orchestration | `src-tauri/src/native_v2_orchestration.rs` |
| Managed Worker coordination | `src-tauri/src/managed_worker_coordinator.rs` |
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

## Provider configuration facts

The Host service stores non-secret provider id, generation, config digest, HTTPS base URL, model, timeout, output-token limit, health, and timestamps in SQLite. The API key is stored separately as authenticated ciphertext under the existing Host master key. Exact generations are immutable run bindings; update increments generation, delete revokes active bindings, and stale references fail closed.

Accepted production endpoints must use HTTPS and valid bounded model/config values. Provider health has `unknown`, `healthy`, and `unhealthy` states. The health probe performs no Worker task effect. Local Tauri commands `get_managed_worker_provider_settings`, `create_managed_worker_provider`, `update_managed_worker_provider`, `delete_managed_worker_provider`, `select_managed_worker_provider`, and `check_managed_worker_provider_health` return only the renderer-safe settings snapshot. Create/update receive a credential only for that command; no list/status/read operation or command returns it. Environment-variable provider loading remains limited to an ignored opt-in development smoke test.

The corresponding product configuration/selection/health closure is implemented. A real external provider/API call remains pending validation; neither configuration nor a bounded health observation alone claims a Managed E2E `PASS`.

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
| Effects/results | Rust `effect_authority`, `managed_resources`, `execution_world`, `network_broker`, and `managed_execution` tests; opt-in native Windows `windows_execution_world` integration test |
| Layer 4 and transfer | `scripts/run-layer4-validation-matrix.mjs`, `scripts/run-transfer-planner-tests.mjs`, Rust transport/protocol tests |
| Developer Terminal | Rust terminal/HostRuntime tests plus native physical platform checks |

The full contributor and physical validation procedure is in [development](development.md).
