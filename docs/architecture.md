# Pastey architecture

Pastey is a local-first desktop transfer and device workspace. Source code is authoritative for current behavior. Version 1.9.2 is the previous structural freeze point for the Layer 1–5 semantic and authority contracts. Version 1.9.3 preserves them while delivering the Phase 1–5 Host, representation, and generic managed-authority foundations. Version 2.0.0 is reserved for the planned Phase 6 managed Agent product milestone and is not implemented.

## Five layers

| Layer | Responsibility |
| --- | --- |
| 1 — Secure LAN transport | Encrypted payload transfer, framing, integrity, acknowledgement, and finalization. |
| 2 — Device intelligence | Factual device, link, liveness, and capability observations. |
| 3 — Smart orchestration | Ordinary queue planning plus the shared Rust capacity/window boundary used by ordinary and managed Transfers. |
| 4 — Bridge | Current-session peers, routes, encrypted control delivery, reconnect, replay, and Burn boundaries. |
| 5 — Managed semantic workspace | Four-primitive object flow, immutable revisions, approval, attempt/step authority, and Host-side continuation. |

Layer 2 facts are observations, not commands or authority. Layer 4 delivery is not consent. Layer 5 binds one complete immutable Plan to one requester Review & Run. Renderer state, provider output, ObjectRefs, and logs are never authority.

Developer Mode v0 is an upper Host capability domain parallel to Layer 5. It reuses Layer 4 identity/session/encrypted control and Burn lifecycle but requires separate two-human admission and a dedicated terminal grant. It creates no Plan step or managed revision. See [Developer Mode](developer-mode.md).

Bridge departure is also current-session and authenticated. Temporary disconnect retains membership for reconnect; explicit leave/Burn removes only the departing peer from the survivor's current membership and revokes authority bound to that peer. Burn remains local destructive cleanup and cannot be requested remotely as a control shortcut.

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

Transform and Execute remain product-unavailable. Plan v2 and the crate-private managed-execution seam can represent exact step authority and validate Host evidence/results, but no Worker, PM, v2 product dispatcher, or live Transform/Execute caller exists. Starting a product revision containing either primitive still fails closed before execution admission with a clear Agent-unavailable error. Schema or substrate presence is never reported as product execution availability.

That fail-closed rule is enforced independently at requester command admission, store-level attempt creation, and receiver protocol admission. A modified caller or authenticated peer cannot use an otherwise valid Search start to partially execute a reviewed Transform/Execute Plan.

## Immutable revision and dependency model

The Composer authors an ordered dependency flow. Rust lowers it without inserting steps. The semantic revision hash covers device bindings, Transfer topology, logical object revisions, modification intent, and execution intent. Editing any of those semantics invalidates the prior unapproved revision.

The current Composer creates logical object `selected_file` revision 1 through a Search-first flow. Each Transform consumes the exact current logical revision and declares the next revision at the same location. Execute consumes the exact current revision and does not create a filesystem result. Because Transform is not executable yet, its declared next revision is a Plan dependency, not a claim that bytes were changed.

Search-first composition and `selected_file` are v1 product constraints, not ontology-level rules. The implemented generic managed-object binder can acquire a safely validated Search result, Inbox item, drag/drop object, local selection, or generated artifact without adding a fifth primitive; Plan v2 roots can represent the resulting exact revision and Host location. Search remains the managed behavior for finding.

`PipelinePrivate` remains an implementation detail only for an explicit intermediate Transfer. It never appears as hidden movement or as a Transform output mechanism. Completion returns to the Layer 5 Host coordinator, which reads immutable attempt state and atomically claims the next dependency-eligible authored Transfer exactly once. It does not assume Transform follows; multiple authored Transfer steps may exist in one attempt.

The execution dependency is:

```text
Layer 5 semantic eligibility
→ Layer 3 shared capacity admission
→ Layer 4 current authenticated/session context
→ Layer 1 encrypted byte transfer
```

Layer 3 cannot create, reorder, approve, or inspect private Plan steps. Layer 1 and Layer 4 do not decide which semantic primitive comes next.

## Security and lifecycle foundations

Rust retains paths, candidate identity, BLAKE3 digest, platform file identity, and ObjectRefs privately. The shared safe-file identity layer uses descriptor-oriented no-follow traversal on Unix and no-reparse component/final-handle validation on Windows, including volume/file index and link-count checks. Search, direct Transfer, and PipelinePrivate consumption reuse this physical identity path and revalidate the exact private source before use.

Bridge/session binding, Plan/revision/attempt/step correlation, replay protection, TTL, one-use Search/Transfer authority, restart interruption, and Burn invalidation remain enforced. Candidate selection selects data and does not approve another action. The receiver has no repeated Allow, Apply, or Run controls.

## Capability projection

The current Host projection contains no concrete Transform or Execute capabilities. Projections contain `0..N` bounded facts, and an empty projection is valid: it does not mean offline, malformed, authorized, or eligible, and no fallback fact is fabricated. Capability facts answer only whether a Host currently performs a capability; they never authorize an action, select a Host, move an object, or rewrite the Plan.

## Version boundary

Structurally frozen after 1.9.2 are the four primitive meanings, explicit Transfer-only movement, immutable reviewed topology, logical revision semantics, semantic approval, non-authoritative provider/renderer/capability facts, route-not-consent separation, Layer 5 eligibility before Layer 3 capacity, exact attempt/step authority, safe physical identity, restart/session/Burn invalidation principles, and separation of Agent authority from Developer Terminal authority.

Version 1.9.3 adds the UI-independent `HostRuntime`, durable Host identity/participants/session binding, exact Host admission, generic managed objects, native Plan/protocol v2, generic effect authority, Host-private resources, execution worlds, an independent network broker, and the crate-private Core managed-execution/result seam. V1 is not reinterpreted and remains the executable UI path. The macOS execution world is implemented; Linux and Windows remain unavailable rather than falling back. Developer Mode v0 remains a separate terminal authority domain.

Version 2.0.0 begins the planned Phase 6 product milestone: Worker Harness, live managed Transform/Execute coordination, PM/planner integration, v2 product/UI flow, and related Agent product work. Those capabilities are not implemented in 1.9.3. Configurable/headless policy, richer Terminal work, and Headless deployment remain separately evolvable. See the canonical [upper architecture](upper-architecture.md) for the dependency order.

## Evidence boundary

Automated tests validate the framework and current Search/Transfer behavior. Cross-compilation is not native platform behavior, and no automated result is physical Mac↔Windows E2E proof. See [Layer 5](layers/layer-5-agent.md), [reference](reference.md), and [development](development.md).

The implemented foundation and planned product/runtime domains above the frozen Layer 1–5 contracts are defined only in the canonical [upper architecture](upper-architecture.md). Other documents link to it rather than duplicating that design.
