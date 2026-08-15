# Pastey architecture

Pastey is a local-first desktop transfer and device workspace. Source code is authoritative for current behavior.

## Five layers

| Layer | Responsibility |
| --- | --- |
| 1 — Secure LAN transport | Encrypted payload transfer, framing, integrity, acknowledgement, and finalization. |
| 2 — Device intelligence | Factual device, link, liveness, and capability observations. |
| 3 — Smart orchestration | Transfer planning, scheduling, capacity, and queue lifecycle. |
| 4 — Bridge | Current-session peers, routes, control transport, reconnect, replay, and Burn boundaries. |
| 5 — Agent-assisted workspace | Guided Plan composition, immutable revisions, approval, object-flow validation, and future Agent integration. |

Layer 2 facts are observations, not commands or authority. Layer 4 delivery is not consent. Layer 5 binds one complete immutable Plan to one requester Review & Run. Renderer state, provider output, ObjectRefs, and logs are never authority.

## Canonical object-flow model

```text
Search     = find
Transform  = modify
Transfer   = move
Execute    = run
```

Every execution location, mutation intent, execution intent, and movement must appear in the approved immutable Plan. Search discovers an object at an explicit Host. Transform authorizes reviewed modification intent on the object's current Host and conceptually advances the same logical object revision. Transfer is the only primitive that changes location or landing. Execute consumes the exact current revision on an explicit Host.

Valid framework flows include:

```text
Search @ B → Transform @ B
Search @ B → Transform @ B → Execute @ B
Search @ B → Transfer B → A → Transform @ A
Search @ B → Transform @ B → Transfer B → A
Search @ B → Transfer B → A → Transform @ A → Execute @ A
```

A cross-device Transform or Execute is invalid unless an explicit earlier Transfer moved the object. Capability observations never repair or rewrite authored topology.

## Current implementation boundary

Search and Transfer are implemented. Their existing encrypted transfer, safe candidate selection, one-click Review & Run, and Rust-owned continuation remain live.

Transform and Execute are Plan-framework primitives only. The Plan stores their target revision, explicit Host, and reviewed intent. Pastey Core does not choose a patch format, mutation adapter, runtime, shell, process, workspace, or containment policy. Starting any revision containing Transform or Execute fails closed before attempt creation or approval consumption with a clear Agent-unavailable error. Schema presence is never reported as execution availability.

## Immutable revision and dependency model

The Composer authors an ordered dependency flow. Rust lowers it without inserting steps. The semantic revision hash covers device bindings, Transfer topology, logical object revisions, modification intent, and execution intent. Editing any of those semantics invalidates the prior unapproved revision.

Search creates logical object `selected_file` revision 1 at its Host. Each Transform consumes the exact current logical revision and declares the next revision at the same location. Execute consumes the exact current revision and does not create a filesystem result. Because Transform is not executable yet, its declared next revision is a Plan dependency, not a claim that bytes were changed.

`PipelinePrivate` remains an implementation detail only for an explicit intermediate Transfer. It never appears as hidden movement or as a Transform output mechanism.

## Security and lifecycle foundations

Rust retains paths, candidate identity, BLAKE3 digest, platform file identity, and ObjectRefs privately. The shared safe-file identity layer uses descriptor-oriented no-follow traversal on Unix and no-reparse component/final-handle validation on Windows, including volume/file index and link-count checks. Search and Transfer revalidate the exact private source before encrypted transfer.

Bridge/session binding, Plan/revision/attempt/step correlation, replay protection, TTL, one-use Search/Transfer authority, restart interruption, and Burn invalidation remain enforced. Candidate selection selects data and does not approve another action. The receiver has no repeated Allow, Apply, or Run controls.

## Capability projection

The current Host projection contains no concrete Transform or Execute capabilities. The generic bounded transport remains available for future Agent-owned observations. Capability facts answer only whether a Host currently performs a capability; they never authorize an action, select a Host, move an object, or rewrite the Plan.

## Evidence boundary

Automated tests validate the framework and current Search/Transfer behavior. Cross-compilation is not native platform behavior, and no automated result is physical Mac↔Windows E2E proof. See [Layer 5](layers/layer-5-agent.md), [reference](reference.md), and [development](development.md).
