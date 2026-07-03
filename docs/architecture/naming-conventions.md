# Pastey Naming Conventions

This document is the canonical naming guide for Pastey architecture, schemas, protocols, Agent Bridge capability contracts, provider action kinds, executor kinds, template kinds, and future capability proposals. It is a naming specification only; it does not add runtime behavior.

When naming appears in code, tests, and docs, source code and tests are the final evidence for implemented behavior. Documentation should link here instead of repeating the full policy.

## Summary Table

| Type | Naming rule | Example |
| --- | --- | --- |
| Large protocol / wire format | `name-vN` | `binary-v1` |
| `schemaVersion` | `domain-purpose-vN` | `filesystem-find-file-candidates-result-v1` |
| Capability ID | no embedded version; use registry `version` field | `filesystem.find_file_candidates` |
| Registry version | `"vN"` | `version: "v1"` |
| Legacy capability | may remain unversioned | `runtime.execute_hello_template` |
| Template kind | snake_case capability-family label | `candidate_payload_handoff` |

## Large Protocol And Wire Format Names

Use `name-vN` for broad protocol or wire-format generations. This form is reserved for large formats that affect a whole transport or framing generation, not every internal message.

Good examples:

- `binary-v1`;
- a future whole-protocol frame format or transfer wire generation.

Do not rename `binary-v1` merely because it contains `-v1`; it is a protocol generation name, not a small schema contract.

## `schemaVersion` Strings

Use kebab-case plus `-vN`.

Rules:

- No slash form such as `/v1`.
- No dotted capability ID format.
- Name the schema's data contract, not the implementation file.
- Use capability-specific names for capability-specific payloads.

Good examples:

- `ai-action-plan-v1`;
- `ai-context-snapshot-v1`;
- `pastey-room-control-event-v1`;
- `pastey-peer-consent-binding-v1`;
- `filesystem-find-file-candidates-result-v1`;
- `transfer-request-candidate-payload-request-v1`.

Bad examples:

- slash-version schema names;
- dotted capability IDs used as schema names;
- endpoint-style schema names;
- broad `pastey-capability-request` names for capability-specific payloads.

Avoid broad schema names such as `pastey-capability-request-v1` when the payload is specific to one capability. Prefer a capability-specific schema such as `pastey-hello-peer-request-v1`.

## Capability IDs

Capability IDs identify what the capability is. They must not embed versions.

Rules:

- Use dotted namespace plus snake_case action or object name.
- Put the version in the registry `version` field.
- Keep capability IDs stable enough for exact consent, audit, and dispatch checks.

Good examples:

- `runtime.execute_hello_template`;
- `runtime.hello_stdout`;
- `filesystem.find_file_candidates`;
- `transfer.request_candidate_payload`;
- `workspace.request_payload_from_candidate`.

Bad examples:

- slash-version capability IDs;
- capability IDs that embed registry versions;
- filesystem-namespaced transfer authority IDs;
- `pastey-runtime-hello-stdout-request-v1` as a capability ID.

## Registry Versions

Every Agent Bridge capability registry contract carries an explicit version.

Correct:

```ts
capability: "filesystem.find_file_candidates",
version: "v1"
```

Incorrect:

```ts
capability: "filesystem.find_file_candidates.v1",
version: "v1"
```

Legacy capabilities may use `version: "legacy"`. Do not duplicate the version in the capability ID.

## Legacy Capability Handling

Existing legacy IDs may remain unversioned when they are already part of a bounded contract.

Current allowed legacy example:

- `runtime.execute_hello_template`.

Do not use legacy naming as the model for new capabilities. New capabilities should use dotted namespace plus snake_case, with no embedded version.

## Provider Action Kinds

Provider action kinds are AI advisory action labels. They are not capability IDs and not schema versions.

Rules:

- Use snake_case.
- Do not include versions.
- Prefer user-intent wording.
- Keep them advisory: a provider action kind does not authorize execution or transfer.

Good examples:

- `request_peer_file_candidates`;
- `request_peer_candidate_payload`;
- `request_peer_hello_demo`;
- `request_peer_hello_stdout_demo`.

Avoid vague or over-authoritative names:

- prepare-style transfer names;
- `do_transfer`;
- `execute_file`.

## Executor Kinds

Executor kinds describe implementation class or path, not product semantics.

Rules:

- Use snake_case.
- Do not include versions.
- Do not expose executor kind as a capability ID or user-facing action name.
- Do not let the model choose executor kinds.

Good examples:

- `rust_host_helper`;
- `filesystem_find_candidates_host`;
- `transfer_candidate_payload_host`;
- `ts_in_process_fixed_template`.

## Template Kinds

Template kinds describe reusable Agent Bridge lifecycle families. They are not capability IDs, schema versions, provider action kinds, executor kinds, or permission grants.

Rules:

- Use snake_case.
- Do not include versions.
- Name the repeated lifecycle/policy shape, not a concrete capability.
- Keep template kinds internal and host-owned.
- Do not let the provider choose template kinds.

Current design examples:

- `bounded_runtime_action`;
- `metadata_discovery`;
- `candidate_payload_handoff`;
- `future_receiver_local_operation`.

Template kind names must not imply unbounded authority. A template can centralize route, consent, request-hash, expiry, forbidden-field, redaction, and typed-result checks, but each concrete capability still needs an explicit manifest and capability-specific validators.

Approval policy names use snake_case and do not grant authority by themselves. Current template implementation uses `never_auto_approve` for actions or prompts that must not be auto-approved.

## Future Candidate-Payload Transfer Naming

Avoid filesystem-namespaced or prepare-style transfer names for candidate payload requests.

Reasons:

- `prepare` is vague.
- It sounds like the system is preparing transfer internally.
- The intended semantics are receiver-mediated authorization/request for one specific candidate payload.
- Search consent is not send consent.

Preferred future capability candidates:

```text
transfer.request_candidate_payload
workspace.request_payload_from_candidate
```

Preferred current direction:

```text
transfer.request_candidate_payload
```

Reasons:

- It says the sender is requesting a candidate payload.
- It fits the transfer namespace.
- It does not imply automatic transfer.
- It separates search consent from send consent.
- It can later hand off to the existing transfer pipeline only after receiver Allow once.

Current schema names for that scaffold:

```text
transfer-request-candidate-payload-request-v1
transfer-request-candidate-payload-consent-grant-v1
transfer-request-candidate-payload-execution-request-v1
transfer-request-candidate-payload-result-v1
```

`transfer.request_candidate_payload` is currently a second-consent queue-handoff path. It validates request/consent/execution/result contracts, resolves selected candidates through the receiver-local in-memory store, and enqueues the resolved file through the existing transfer scheduler. `handoff_queued` does not mean transfer completion, and the result schema still does not expose local paths, file contents, queue ids, or handoff ids.

## Namespace Guidance

- `runtime.*`: bounded runtime, demo, or helper capabilities.
- `filesystem.*`: filesystem metadata discovery or local filesystem facts, not transfer authority.
- `transfer.*`: request, authorization, or handoff semantics related to payload transfer.
- `workspace.*`: higher-level product workflow capabilities when an operation spans multiple subsystems.
- `pastey-*`: schemaVersion namespace, not capability ID namespace.
- `ai-*`: AI advisory/context schema namespace.
- `bridge-*` or `pastey-room-control-*`: Bridge and room-control schema namespaces.

## Safety Naming Rules

Names must not imply stronger authority than the capability actually has.

Rules:

- A search capability must not be named as if it can fetch or send files.
- A preview schema must not be named as execution.
- A consent grant must name the exact capability it authorizes.
- A candidate ID must not be named or displayed as a path.
- A future transfer request capability must emphasize request and authorization, not automatic send.

## Anti-Patterns

- Embedding slash-version suffixes in capability IDs.
- Mixing capability IDs and schemaVersion names.
- Using endpoint-like version suffixes for internal contracts.
- Using vague verbs such as `prepare`, `handle`, `process`, or `do`.
- Naming a future capability before its consent and executor boundaries are defined.
- Reusing broad names like `pastey-capability-request-v1` for capability-specific schemas.
- Renaming `binary-v1` merely because it contains `-v1`.

## Migration Checklist

Use this checklist for future PRs that add or rename schemas, capabilities, provider action kinds, executors, or protocol names:

- Capability ID has no version.
- Registry has explicit `version`.
- `schemaVersion` uses `-vN`, not endpoint-style suffixes.
- Template kind, if present, is snake_case and names a lifecycle family rather than authority.
- Provider action kind has no version.
- Executor kind has no version.
- Docs use the same names as code.
- Tests assert exact names.
- Internal schema and capability grep checks return no endpoint-style version matches.
- No new capability overclaims authority in its name.
