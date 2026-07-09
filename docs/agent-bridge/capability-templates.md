# Capability Templates

This document designs and tracks the template-based Layer 5 capability architecture for Pastey Agent Bridge. Phase 1 static manifests, Phase 2 additive helpers, the Phase 3 `runtime.hello_stdout` wrapper, the Phase 4 `filesystem.find_file_candidates` common-check wrapper, the Phase 5 `transfer.request_candidate_payload` common-check wrapper, and the Phase 6 deterministic candidate workflow are implemented for the current narrow capability set. Pastey 1.9.1 completes the current Layer 5 narrow product closure through Ask Bridge natural-v1 Search / Return, folds Request file into that plan model, keeps Hello Stdout diagnostic/test-only, and consolidates smoke bug fixes around target binding, Deny propagation, automatic active-operation refresh, device platform display, full content access, and product-status documentation. This is not full Agent Bridge or full Jarvis completion. The current implementation does not change public capability IDs, schema versions, provider action kinds, executor kinds, room-control event names, transfer queue APIs, `binary-v1`, or existing validators.

For the current capability contracts, see [capability-contracts.md](capability-contracts.md). For the broader safety model, see [architecture-and-safety.md](architecture-and-safety.md). For capability ID, schema, provider action, executor, and template naming rules, see [../architecture/naming-conventions.md](../architecture/naming-conventions.md).

## Design Goal

Layer 5 capabilities should be built from reusable templates plus explicit manifests instead of designing every new capability from scratch. A template captures repeated lifecycle and policy shape. A manifest binds that shape to one explicitly registered capability.

Templates are not generic tools. They do not load plugins, accept provider-supplied capabilities, dispatch arbitrary executor names, or weaken capability-specific validators. Source code, validators, tests, and direct runtime behavior remain the source of truth for implemented behavior.

The highest-autonomy Pastey mode must still be:

- capability-bound;
- route-bound;
- scope-bound;
- session-bound;
- consent/policy-bound;
- without a general shell;
- without arbitrary path access;
- without AI-visible local paths;
- without AI-visible file contents.

Pastey has no true full-access mode. Any future higher-autonomy mode must remain bounded and revocable.

## Capability Template

A `CapabilityTemplate` describes the repeated lifecycle shape for a family of explicitly registered capabilities:

```text
advisory
-> local preview
-> route validation
-> receiver preview
-> approval/consent
-> execution request
-> executor
-> typed result
-> audit/event record
```

The template supplies shared checks and helper behavior. It is not a universal executor and is not sufficient to accept a request by itself. Each concrete capability still needs an explicit manifest, schema-specific input validator, schema-specific execution validator, executor-specific validation, result validator, and tests.

Suggested template kinds:

```ts
type CapabilityTemplateKind =
  | "bounded_runtime_action"
  | "metadata_discovery"
  | "candidate_payload_handoff"
  | "future_receiver_local_operation";
```

Template shape:

```ts
type CapabilityTemplate = {
  templateKind: CapabilityTemplateKind;
  lifecycle: {
    advisory: boolean;
    localPreview: boolean;
    receiverPreview: boolean;
    consent: "none" | "allow_once" | "session_policy";
    execution: boolean;
    result: boolean;
  };
  routePolicy: "local-only" | "selected-peer";
  dataExposurePolicy:
    | "metadata_only"
    | "local_only_source"
    | "payload_queue_internal";
  sideEffectClass:
    | "no_side_effect"
    | "local_metadata_read"
    | "remote_metadata_read"
    | "remote_runtime_action"
    | "payload_handoff";
  defaultApprovalPolicy: AgentBridgeApprovalPolicy;
  requiredBindings: string[];
  forbiddenFields: string[];
};
```

Shared template checks should cover:

- schemaVersion naming shape checks;
- capability ID naming shape checks;
- selected-peer route enforcement;
- selected-peers and broadcast rejection for capability events;
- exact request hash binding;
- consent grant binding;
- expiry checks;
- one-time consent consumption;
- forbidden public fields;
- audit metadata redaction;
- typed result envelope requirements.

Capability-specific code still owns:

- input schema;
- safe scope validation;
- candidate store lookup;
- runtime executor behavior;
- payload handoff behavior;
- result data shape.

## Template Families

| Template kind | Intended use | Default route | Data exposure | Side-effect class | Default consent |
| --- | --- | --- | --- | --- | --- |
| `bounded_runtime_action` | Fixed host-owned runtime/demo actions such as Hello Peer and Hello Stdout | `selected-peer` | `metadata_only` | `remote_runtime_action` | `allow_once` |
| `metadata_discovery` | Receiver-local metadata discovery without file contents or path exposure | `selected-peer` | `metadata_only` | `remote_metadata_read` | `allow_once` |
| `candidate_payload_handoff` | Second-consent handoff for one previously discovered candidate into the existing transfer queue | `selected-peer` | `payload_queue_internal` | `payload_handoff` | `allow_once` |
| `future_receiver_local_operation` | Reserved future shape for bounded receiver-local operations with explicit scope design | `selected-peer` or `local-only` | capability-specific | capability-specific | `allow_once` or `session_policy` |

`future_receiver_local_operation` is intentionally reserved. It is not an implemented capability family and must not be used to justify arbitrary filesystem access, file contents exposure, network calls, model-authored scripts, arbitrary process execution, or provider-chosen executors.

## Capability Manifest

Each concrete capability declares a manifest. Existing capability manifests must be representable without changing current public behavior.

```ts
type CapabilityManifest = {
  capability: string;
  version: "v1" | "legacy";
  templateKind: CapabilityTemplateKind;
  providerActionKind: string;
  executorKind: string;

  routePolicy: "local-only" | "selected-peer";
  consentPolicy: "none" | "exact-allow-once" | "session-bound-policy";
  dataExposurePolicy:
    | "metadata_only"
    | "local_only_source"
    | "payload_queue_internal";
  auditRedactionPolicy: "metadata_only" | "local_only" | "queue_internal";

  schemaVersions: {
    advisory?: string;
    request: string;
    consentGrant?: string;
    executionRequest?: string;
    result: string;
  };

  autonomySupport: {
    manual: boolean;
    assisted: boolean;
    trustedSession: boolean;
  };

  approvalRequirements: {
    localUserConfirm: boolean;
    receiverAllowOnce: boolean;
    allowSessionPolicy: boolean;
    allowAutoReview: boolean;
  };

  safety: {
    selectedPeerOnly: boolean;
    rejectsBroadcast: boolean;
    rejectsSelectedPeers: boolean;
    forbidsAbsolutePathExposure: boolean;
    forbidsContentExposure: boolean;
    forbidsGenericExecution: boolean;
  };
};
```

The implementation keeps manifests static and host-owned in `src/lib/agentBridge/capabilityManifest.ts`. The provider cannot define a manifest, choose an executor kind, or override manifest safety flags. The existing registry in `src/lib/ai/capabilityRegistry.ts` remains authoritative for dispatch and validation.

## Existing Capability Adapter Mapping

This mapping is descriptive. It preserves the existing capability IDs, provider action kinds, executor kinds, schemaVersion strings, validators, routes, consent behavior, room-control event names, and tests.

| Capability | Version | Template kind | Provider action kind | Executor kind | Route/consent | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `runtime.execute_hello_template` | `legacy` | `bounded_runtime_action` | `request_peer_hello_demo` | `ts_in_process_fixed_template` | `selected-peer`, exact Allow once | Legacy fixed Hello Peer capability. Keep ID unrenamed. |
| `runtime.hello_stdout` | `v1` | `bounded_runtime_action` | `request_peer_hello_stdout_demo` | `rust_host_helper` | `selected-peer`, exact Allow once | First capability wrapped with manifest-backed constants plus shared exact capability, request-hash, and expiry helpers. |
| `filesystem.find_file_candidates` | `v1` | `metadata_discovery` | `request_peer_file_candidates` | `filesystem_find_candidates_host` | `selected-peer`, exact Allow once | Metadata only. Receiver-local scope labels only. No content, no absolute paths, no transfer authority. Wrapped with manifest-backed constants, exact binding helpers, expiry checks, and forbidden public-field checks while keeping filesystem validators bespoke. |
| `transfer.request_candidate_payload` | `v1` | `candidate_payload_handoff` | `request_peer_candidate_payload` | `transfer_candidate_payload_host` | `selected-peer`, exact Allow once | Second consent for one prior candidate. Queue acceptance returns `handoff_queued`; transfer completion remains owned by the existing transfer pipeline. Wrapped with manifest-backed constants, exact binding helpers, expiry checks, and forbidden public-field checks while keeping candidate resolution and queue handoff bespoke. |

Adapter-compatible migration target:

```text
old explicit capability implementation
-> wrapped by template manifest
-> same public behavior
```

## Example Manifest Sketches

These sketches are future internal shapes. They are not new runtime code.

```ts
const helloStdoutManifest: CapabilityManifest = {
  capability: "runtime.hello_stdout",
  version: "v1",
  templateKind: "bounded_runtime_action",
  providerActionKind: "request_peer_hello_stdout_demo",
  executorKind: "rust_host_helper",
  routePolicy: "selected-peer",
  consentPolicy: "exact-allow-once",
  dataExposurePolicy: "metadata_only",
  auditRedactionPolicy: "metadata_only",
  schemaVersions: {
    request: "pastey-runtime-hello-stdout-request-v1",
    consentGrant: "pastey-runtime-hello-stdout-consent-grant-v1",
    executionRequest: "pastey-runtime-hello-stdout-execution-request-v1",
    result: "pastey-runtime-hello-stdout-execution-result-v1",
  },
  autonomySupport: {
    manual: true,
    assisted: true,
    trustedSession: false,
  },
  approvalRequirements: {
    localUserConfirm: true,
    receiverAllowOnce: true,
    allowSessionPolicy: false,
    allowAutoReview: false,
  },
  safety: {
    selectedPeerOnly: true,
    rejectsBroadcast: true,
    rejectsSelectedPeers: true,
    forbidsAbsolutePathExposure: true,
    forbidsContentExposure: true,
    forbidsGenericExecution: true,
  },
};
```

```ts
const candidatePayloadManifest: CapabilityManifest = {
  capability: "transfer.request_candidate_payload",
  version: "v1",
  templateKind: "candidate_payload_handoff",
  providerActionKind: "request_peer_candidate_payload",
  executorKind: "transfer_candidate_payload_host",
  routePolicy: "selected-peer",
  consentPolicy: "exact-allow-once",
  dataExposurePolicy: "payload_queue_internal",
  auditRedactionPolicy: "metadata_only",
  schemaVersions: {
    request: "transfer-request-candidate-payload-request-v1",
    consentGrant: "transfer-request-candidate-payload-consent-grant-v1",
    executionRequest: "transfer-request-candidate-payload-execution-request-v1",
    result: "transfer-request-candidate-payload-result-v1",
  },
  autonomySupport: {
    manual: true,
    assisted: true,
    trustedSession: false,
  },
  approvalRequirements: {
    localUserConfirm: true,
    receiverAllowOnce: true,
    allowSessionPolicy: false,
    allowAutoReview: false,
  },
  safety: {
    selectedPeerOnly: true,
    rejectsBroadcast: true,
    rejectsSelectedPeers: true,
    forbidsAbsolutePathExposure: true,
    forbidsContentExposure: true,
    forbidsGenericExecution: true,
  },
};
```

## Autonomy Profiles

Autonomy profiles describe how much prompt friction the host may reduce for bounded capabilities. They do not grant unbounded authority.

```ts
type AgentBridgeAutonomyProfile =
  | "manual"
  | "assisted"
  | "trusted_session";
```

### `manual`

Default mode.

- Every side-effecting capability requires explicit local confirmation and receiver Allow once.
- No automatic candidate selection.
- No automatic payload handoff without receiver Allow once.
- Matches current public behavior.

### `assisted`

For trusted paired devices where the user still drives sensitive choices.

- AI may propose multi-step workflows.
- Host may prepare the next preview automatically after a previous bounded result.
- User must select a candidate before payload request.
- Receiver still chooses Allow once for payload transfer.
- No automatic file/content selection.

### `trusted_session`

Specification-only for most capabilities until separately implemented and tested.

- Session-bound reduced prompts for trusted peers and safe scopes.
- Still no arbitrary path access.
- Still no shell or open-ended tool execution.
- Still no AI-visible path or file contents.
- Still capability-bound, route-bound, scope-bound, consent/policy-bound, and revocable.

Do not introduce modes named after unbounded disk or unrestricted authority. High-autonomy language should use `trusted_session` and must still state its limits.

## Approval Policy And Reviewer

Approval policy is a separate axis from autonomy profile.

```ts
type AgentBridgeApprovalPolicy =
  | "always_ask"
  | "ask_on_sensitive"
  | "session_bound"
  | "never_auto_approve";

type AgentBridgeApprovalReviewer =
  | "user"
  | "policy_gate"
  | "auto_review";
```

Policy meanings:

- `always_ask`: every side-effecting request requires user or receiver approval.
- `ask_on_sensitive`: metadata-only preview may be prepared automatically, but payload handoff still asks.
- `session_bound`: only for trusted session rules; still bounded and revocable.
- `never_auto_approve`: explicit marker that some prompts/actions must never be auto-approved.

Reviewer rules:

- `policy_gate` can reject unsafe actions automatically.
- `auto_review` can only reduce nuisance prompts for actions that are already safe, already bounded, schema-valid, route-valid, and manifest-allowed.
- `auto_review` must not approve arbitrary path access, file content exposure, open-ended execution, broadcast side effects, or selected-peers side effects.
- User/receiver approval remains required for payload transfer in `manual` and `assisted` modes.

## Generated Lifecycle Helpers

The Phase 2 helper implementation in `src/lib/agentBridge/capabilityTemplateHelpers.ts` centralizes these checks:

| Shared helper | Purpose |
| --- | --- |
| `assertCapabilityNaming` | Ensures capability IDs and schema versions follow the canonical naming rules. |
| `assertManifestMatchesRegistry` | Ensures manifest values match the existing static registry before a capability can be wrapped. |
| `assertSelectedPeerRoute` | Enforces `selected-peer` routing for capability events. |
| `rejectFanoutRoutes` | Rejects selected-peers and broadcast routes for current capability events. |
| `bindRequestHash` | Binds consent and execution to the exact preview payload. |
| `assertExactCapability` | Verifies that an exact capability binding matches a known manifest. |
| `assertConsentNotExpired` | Verifies consent expiry for helper-wrapped flows. |
| `rejectForbiddenPublicFields` | Rejects paths, contents, command/process fields, queue internals, and result-only fields where disallowed. |

These helpers must be additive. They must not replace the current capability-specific validators in `fileCandidateRequest`, `candidatePayloadRequest`, `helloPeerRequest`, `helloStdoutRequest`, `peerConsent`, or `roomControlEvent`.

## Interface Preservation

The template design must preserve:

- `filesystem.find_file_candidates`;
- `transfer.request_candidate_payload`;
- `request_peer_candidate_payload`;
- `transfer_candidate_payload_host`;
- existing provider action kinds;
- existing executor kinds;
- existing schemaVersion strings;
- room-control event names;
- existing tests;
- `binary-v1`;
- transfer queue API behavior.

Payload transfer remains bytes-oriented and uses the existing transfer queue, scheduler, routing, and binary transfer pipeline. Receiver environment compatibility can be useful display or planning metadata, but it is not transfer authorization. Queue acceptance is not transfer completion.

## Product Closure Status

Pastey 1.9.1 Layer 5 narrow product closure is implemented through Ask Bridge natural-v1:

- Ask Bridge is the single natural-language Layer 5 entry.
- Request file is folded into Ask Bridge as a `Search` / `Return` plan, not a separate primary product model.
- `filesystem.find_file_candidates` plus `transfer.request_candidate_payload` implements Search / Return behind the product primitives. It requires exactly one selected peer, metadata-only search consent, redacted candidates, manual candidate selection, second payload consent, receiver-side candidate revalidation, and queue handoff into the existing transfer pipeline.
- Search -> Transform -> Return may be parsed and previewed in natural-v1, but unsupported transforms fail closed / show unsupported future state until bounded transform runtime exists.
- `runtime.hello_stdout` remains diagnostic/test-only fixed runtime coverage and is no longer user-facing product UI.

The shared `OperationTimeline` product component is an operation lifecycle view backed by Pastey events and existing queue/transfer state. It is not a model reasoning trace, provider scratchpad, or task taxonomy. Active Bridge detail operations auto-refresh while nonterminal; `Check for updates` is retained as fallback/debug affordance only.

Remaining gaps include a global Activity detail drawer, broad two-device smoke validation, transfer-completion smoke for the handoff path, broader capability coverage, durable identity/trust work where explicitly needed, and multi-step orchestration.

### 1.9.1 Manual Smoke Checklist

The automated contracts cover Ask Bridge Search and Return approval/deny boundaries, diagnostic Hello success/deny chains, transport rejection state, serialized automatic refresh, receiver inbox inclusion, and duplicate-send prevention. A real two-device rerun remains required for release evidence:

- Ask Bridge Search: natural-language input, Search / Transform / Return preview, sender confirmation before any peer request, metadata search approval and deny.
- Ask Bridge Return: manual candidate selection, separate candidate-payload approval and Candidate payload deny.
- Hello diagnostics: fixed Hello Peer / Hello Stdout tests remain diagnostic/test-only and are not product UI smoke steps.
- Candidate payload success: `handoff_queued` means queue acceptance only; transfer progress and completion remain owned by the existing transfer pipeline.
- Failure: an unavailable or rejecting peer produces a transport failure and never a delivered state.
- `Check for updates` remains a fallback; normal sender and receiver progression should require no manual refresh.
- Device labeling: a remote Linux peer display does not appear as local `This Mac`.

## Migration Plan

### Phase 0: docs/spec only

- Add this document and pointers from current Agent Bridge and architecture docs.
- No behavior change.
- No package, lockfile, `.prograph`, `binary-v1`, transfer, or runtime source changes.

### Phase 1: manifests for existing capabilities

- Add static manifests for the four existing capabilities.
- Add tests proving manifests match the existing registry and schema contracts exactly.
- Keep old registry and validators authoritative.
- Status: implemented in `src/lib/agentBridge/capabilityManifest.ts` with `tests/capabilityManifest.test.ts`.

### Phase 2: shared helper extraction

- Extract shared route, consent, request-hash, expiry, one-time consumption, and forbidden-field helpers.
- Prove no behavior change through existing tests.
- Do not consolidate capability-specific schema validators into one permissive validator.
- Status: implemented as additive helpers in `src/lib/agentBridge/capabilityTemplateHelpers.ts`.

### Phase 3: migrate `runtime.hello_stdout`

- Wrap `runtime.hello_stdout` first because it has fixed input, fixed executor, and simple result shape.
- Keep public ID, provider action kind, executor kind, schema strings, consent behavior, and tests unchanged.
- Status: implemented only for `runtime.hello_stdout` execution binding. The request schema, consent schema, execution request schema, result schema, Rust executor behavior, stdout/stderr/exit bounds, timeout behavior, and room-control event names are unchanged.

### Phase 4: migrate `filesystem.find_file_candidates`

- Move only common lifecycle checks to template helpers.
- Keep safe-scope, filename, metadata, candidate-result, and receiver-local store validation capability-specific.
- Status: implemented for common lifecycle checks only. Safe scope validation, query validation, filename and extension filtering, maxDepth and maxCandidates bounds, hidden-file and symlink skip behavior, metadata-only result validation, candidate id opacity checks, receiver-local candidate store writes, and Rust Tauri command validation remain capability-specific.

### Phase 5: migrate `transfer.request_candidate_payload`

- Migrate only after candidate-payload consent, handoff, transfer scheduler, and forbidden-field tests remain green.
- Keep source discovery binding and queue handoff behavior capability-specific.
- Status: implemented for common lifecycle checks only. Source discovery binding, `sourceRequestId + candidateId + candidateKind` binding, receiver-local candidate lookup, missing/expired/changed/deleted candidate handling, queue handoff, Agent Bridge queue metadata, `handoff_queued` / `handoff_failed` result behavior, no-path exposure checks, transfer scheduler interaction, and candidate payload result shape remain capability-specific.

### Phase 6: deterministic candidate workflow

- Chain the existing `filesystem.find_file_candidates` and `transfer.request_candidate_payload` capabilities through a deterministic host-owned workflow.
- Keep AI output advisory only and host validation authoritative.
- Require explicit user candidate selection before any payload request preview is built.
- Keep receiver Allow once for both discovery and payload handoff.
- Preserve `handoff_queued` as queue acceptance, not transfer completion.
- Status: implemented in `src/lib/agentBridge/candidatePayloadWorkflow.ts` with no new capability IDs, no generic executor, no auto-send behavior, no trusted-session runtime behavior, and no path/content exposure.

### Future phases

- Require every future capability to declare a manifest and template kind before implementation.
- Reuse the workflow model only where discovery, candidate selection, second consent, and transfer handoff boundaries remain explicit.

## Proposed Tests

Future implementation should add coverage that proves:

- manifests validate all existing capabilities;
- template default policies match current bespoke policies;
- `manual` profile preserves current behavior;
- `assisted` profile cannot auto-select a candidate;
- `trusted_session` cannot expose path or content;
- `metadata_discovery` cannot authorize payload handoff;
- `candidate_payload_handoff` requires source discovery binding;
- selected-peer only remains enforced;
- broadcast and selected-peers reject;
- forbidden fields reject across all template result schemas;
- old capability IDs and schemaVersion strings remain unchanged.

If runtime source changes during a later phase, run the focused Agent Bridge, candidate-payload, transfer planner, and Rust validation stack described in the task-specific validation plan for that phase.
