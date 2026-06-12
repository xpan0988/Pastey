# Advisory Action Plan Boundary

The current AI Slot may produce advisory action plans only. A plan is a
structured explanation or suggestion for the user, not permission to execute.

Mock and cloud preview output pass through the same `validateAiActionPlan` and
`evaluateAiPolicy` functions. Accepted Hello Peer plans may then become a
local-only `PendingAiAction`. Provider output is untrusted regardless of route.
After local confirmation, Phase E0 may convert the fixed Hello Peer proposal
into a validated `HelloPeerRequest` with `transportStatus: "preview_only"`.

## Allowed First-Phase Kinds

| Kind | Purpose | Confirmation boundary |
| --- | --- | --- |
| `explain_status` | Explain current visible Pastey status. | No execution. |
| `summarize_room_state` | Summarize the current redacted room and peer state. | No execution. |
| `summarize_diagnostics` | Summarize current Device Diagnostics/latest benchmark state. | No execution. |
| `explain_transfer_failure` | Explain a sanitized current transfer error and likely user-visible next steps. | No execution. |
| `suggest_retry` | Suggest retrying a selected/current failed transfer. | Explicit confirmation; use the existing UI/queue path. |
| `suggest_benchmark` | Suggest an existing loopback or, in a later UI, peer benchmark. | Explicit confirmation; use the existing benchmark UI/path. |
| `suggest_transfer` | Suggest transferring a user-selected file reference to the current room. | Explicit confirmation; use existing selection/queue and `sendFileToRoom` path. |
| `draft_text_message` | Draft text for the current room composer. | User reviews/edits and explicitly sends through `sendTextToRoom`. |
| `explain_microflowgroup_mode` | Explain current fixed/dynamic mode and visible planner summary. | No execution and no mode-change suggestion. |
| `request_peer_hello_demo` | Propose the restricted Hello Peer design action against the synthetic preview peer. | Required confirmation flag and deny-first policy checks; Phase E0 can build a local request preview, but no request is sent and no execution path exists. |

## Example Documentation Shape

```ts
interface AiActionPlan {
  schemaVersion: "ai-action-plan/v1";
  kind: AiAllowedActionKind;
  title: string;
  explanation: string;
  requiresUserConfirmation: boolean;
  references?: Array<{
    type: "room" | "queue_item" | "selected_file" | "benchmark";
    opaqueSessionId: string;
  }>;
  proposedInput?: {
    draftText?: string;
    benchmarkMode?: "raw_memory" | "pastey_pipeline";
  };
}
```

Action references must be opaque current-session identifiers. A provider must
not receive or return an absolute path, transfer secret, room key, raw command,
Tauri command name, or arbitrary function name.

## Validation And Policy Gate

Before an advisory plan is shown:

1. parse provider output as untrusted data;
2. validate the exact action-plan schema and version;
3. reject unknown action kinds and extra execution-bearing fields;
4. reject paths, secrets, shell text, peer filesystem requests, and unsupported
   actions;
5. bind references only to still-current visible session objects;
6. evaluate the plan through `AiPolicyGate`;
7. clearly label the result as an AI-generated explanation or suggestion.

The policy gate must reject plans that attempt to:

- invoke a Tauri command or internal function directly;
- create, join, leave, or burn a room;
- send text or files automatically;
- cancel transfers automatically;
- change scheduler, MicroFlowGroup, transfer-window, or runtime-window behavior;
- read/search peer or local files;
- run shell commands or peer commands;
- start hidden benchmarks;
- create persistent memory or profiling.

## Confirmation Handoff

Confirmation is a UI decision, not a provider capability. Phase D implements
only a local pending Hello Peer confirmation. It binds the visible canonical
payload, pending ID, and expiry to a deterministic payload hash, then changes
status to `confirmed_local_only`, `cancelled`, or `expired`. It does not
dispatch anything, satisfy peer consent, or provide replay protection for a
transport that does not yet exist.

Phase E0 converts only `confirmed_local_only` into a canonical outbound request
preview and validates it again. A transport request object is not a sent
request. No peer receives the preview, peer consent is not implemented, and
request ID/nonce/expiry/hash fields do not complete replay protection.

Phase E1 wraps that request in a validated
`CapabilityRequestPreviewEnvelope`, checks current-session duplicate envelope
and request IDs, and renders a local inbound-preview simulation. Acknowledge
and deny change preview status only. Actual room transport remains blocked, and
no acknowledge or deny status is returned to a peer.

In a separately reviewed future phase, confirmation may translate a narrow
approved suggestion into an existing user flow:

```text
suggest_transfer
  -> user confirms selected file and destination
  -> existing file selection/queue UI
  -> existing planner
  -> processTransferQueueItem
  -> sendFileToRoom

draft_text_message
  -> user reviews/edits
  -> existing room composer
  -> sendTextToRoom

suggest_benchmark
  -> user confirms
  -> existing Device Diagnostics benchmark control/path
```

The provider never receives the execution result as authority to trigger a
follow-up action. Any future multi-step flow requires a separately reviewed
interaction and threat model.

## Forbidden First-Phase Actions

- direct peer command execution;
- raw shell;
- peer filesystem search or file read;
- hidden file transfer;
- automatic send without user confirmation;
- automatic scheduler or MicroFlowGroup changes;
- automatic transfer-window or runtime-window changes;
- persistent AI memory;
- long-term user behavior profiling.
