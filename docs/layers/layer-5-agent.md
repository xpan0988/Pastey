# Layer 5 — Agent-assisted device workspace

Layer 5 is a bounded, Bridge-scoped workspace for asking one selected peer for help. The model proposes; the host validates; the sender confirms; the receiver chooses Allow once or Deny; a host-owned capability acts; and typed results return through Bridge control transport. Provider output is advisory only.

## Natural-v1: Search / Transform / Return

Ask Bridge is the single natural-language entry. Its `ask-bridge-natural-v1` plan contains one to three ordered `Search`, `Transform`, and `Return` primitives and always requires sender confirmation. The static validator, provider instruction pack, and risk scanner reject unsafe or extraneous model output before it can become a pending action. PolicyGate applies the deny-first capability bounds before a preview is sent.

- **Search** maps to `filesystem.find_file_candidates` for one selected peer. The receiver searches approved scope labels only after Allow once and returns bounded redacted metadata—never file contents or absolute paths.
- **Return** after a user manually selects a candidate maps to `transfer.request_candidate_payload`. It requires a new preview and second receiver Allow once. Receiver-local resolution may queue the existing transfer path; `handoff_queued` is queue acceptance, not transfer completion.
- **Transform** has one bounded kind, `selected_artifact_output`, mapped by host code to `artifact.transform_selected` after manual candidate selection. Other Transform kinds remain `unsupported_future`.

Hello Peer remains fixed diagnostic coverage. `runtime.hello_stdout` is diagnostic/test-only, not a primary product feature.

## Provider and host boundaries

The provider instruction pack is source-controlled (`src/lib/ai/providerInstructionPack.ts`) and is not loaded from Markdown, workspace files, or provider output. The OpenAI-compatible cloud provider receives redacted context only. A provider health check sends a minimal advisory plan and runs the same host validation; it does not send control events or create authority. Deterministic/mock natural-v1 planning remains available without cloud configuration.

The static capability registry and manifest table are host-owned. Capability IDs, route policy, exact allow-once consent policy, public-field rejection, validators, and result contracts are not provider-selectable. Template helpers add shared consistency checks; they do not replace capability-specific validation or make template kinds permissions.

### Provider configuration

Settings owns provider kind, configured OpenAI-compatible base URL and model, enablement, redacted lifecycle-log level, and the runtime-memory API key. The active Bridge owns its current-session preview, consent, execution, and result state. Provider configuration neither creates a durable peer identity nor authorizes an action.

## Consent, routing, and candidates

Every capability preview binds one Bridge session, source peer, target peer, request, payload hash, and expiry. Layer 4 selected-peer transport validates the route, but delivery is not consent. The receiver's Rust-owned consent prompt records the exact binding; one Allow once cannot be reused for another capability, request, or candidate.

`filesystem.find_file_candidates` searches only safe receiver-local scope labels (`downloads`, `desktop`, `documents`, and `pastey_shared` when available), skips hidden entries and symlinks, and returns metadata-only opaque candidate IDs. Candidate IDs are not paths, file handles, or transfer authority. The receiver keeps a TTL-bounded in-memory candidate store; it clears on restart and never exposes its local paths.

`transfer.request_candidate_payload` is a separate second-consent operation. Payload handoff requires second consent. It revalidates the exact receiver-local candidate before queue handoff. Search consent does not authorize transfer, and candidate handoff does not mean bytes transferred or completed.

## Transform authority and results

Rust owns Transform admission, pending consent derivation from an authenticated received preview, candidate lease/identity revalidation, operation ledger/journal, finalization, and authoritative result transport. TypeScript is a UI and validation mirror; it cannot create a Rust approval, receive raw executor output, or send a caller-created Transform result.

The journal stores opaque lifecycle/correlation facts and terminal categories, not paths, source bytes, raw output, digests, or sanitation markers. A future executor must keep raw output in Rust. The sanitizer bounds UTF-8 output and rejects receiver-private path, file-URL, digest, lease, and operation markers before an authoritative result can be sent.

### Implemented and production-active

Natural-v1 planning; Search; Search → selected-file Return; provider validation and risk scanning; sender confirmation; receiver consent; metadata search; candidate-payload second consent; queue handoff; bounded Transform contracts; Rust-owned Transform authority; and result sanitation are implemented.

### Implemented but not production execution

Descriptor-based staging, a static Linux sandbox capability probe, the Stage 2B behavioral-verifier foundation, a feature-gated test probe, mocked verifier tests, and the Rust-private sandbox-adapter seam are implemented. Descriptor staging copies an exact leased receiver-local input into an opaque app-data root within the bounded profile, with a normalized read-only `input/artifact` and separate private work directory. These foundations do not execute a Transform.

### Not yet verified or implemented

Live Linux behavioral verification, delegated cgroup enforcement proof, descendant-containment proof, deterministic worker, gated supervisor, a production Linux sandbox backend, and real Transform execution remain unavailable. The Stage 2B verifier foundation is not a verified isolation substrate.

Production uses `UnavailableTransformSandboxAdapter`. It returns `sandbox_unavailable` before staging, lease, journal mutation, or execution mutation. There is no direct-process fallback. Missing, failed, or indeterminate sandbox prerequisites remain unavailable; macOS, Docker-only, and reduced-isolation fallbacks do not enable Transform.

## Non-goals

Layer 5 does not provide arbitrary shell, process, file, or network execution; model-authored code; automatic candidate selection; automatic file sending; trusted-session execution; durable peer identity as authority; a generic tool/plugin runtime; MCP execution; local-model scheduling; or autonomous task graphs.

For names and source pointers, see [reference.md](../reference.md). For test and live-verification commands, see [development.md](../development.md).
