# Layer 5 — Managed semantic workspace

Layer 5 owns guided Plan composition, optional advisory proposals, immutable object-flow revisions, one complete requester approval, exact attempt/step authority, Host-owned continuation state, and the boundary for future Agent integration. Version 1.9.2 freezes these semantic and authority contracts.

## Product primitives

- **Search** finds and selects an object on the reviewed Host and scopes. It is implemented.
- **Transform** records authorization to modify the selected object on its explicit current Host according to reviewed modification intent. It is framework-only.
- **Transfer** explicitly moves the current revision to another device or landing. It is implemented; `PipelinePrivate` is used only for an authored intermediate Transfer.
- **Execute** records authorization to run the exact current revision on an explicit Host according to reviewed execution intent. It is framework-only.

Only Transfer moves the object. Transform never selects another Host or creates an unrelated derivative. Execute is never hidden inside Transform or Transfer.

## Composer and Review

The primary UX is the editable four-primitive Block Composer. Transform presents Target, Modify on, logical revision, and Modification intent. Execute presents Target, Execute on, logical revision, and Execution intent. Neither presents byte offsets, patch-worker details, interpreter controls, shell commands, arguments, cwd, environment, timeout, or process settings.

Review shows the complete object flow and clearly labels Transform/Execute as framework-defined but not currently executable. One Review & Run remains the only requester approval. The receiver has no per-step approval or run controls.

Natural-v1 uses the same vocabulary. Search/Transfer proposals may be supported; any proposal containing Transform or Execute is marked `unsupported_future` until an Agent implementation exists.

## Object flow and revision semantics

The current Search-first Composer establishes logical object `selected_file` revision 1 at its Host. Transform consumes the exact current logical revision and declares revision N+1 at the same Host. Execute consumes the exact current revision. Transfer preserves the revision while changing its location.

These revision transitions are immutable Plan dependencies. A declared post-Transform revision does not claim production bytes exist. Cross-device Transform or Execute fails validation unless an explicit prior Transfer established locality. No capability fact may insert or remove movement.

Search and `selected_file` are current Composer constraints, not permanent managed-object ontology. Future safe acquisition/binding can import Search results, Inbox files, drag/drop objects, local user selections, or generated artifacts as managed logical revisions without introducing a fifth primitive. The future contract is defined in [upper-architecture.md](../upper-architecture.md).

## Current execution boundary

Search and Transfer retain Rust-owned automatic continuation, candidate selection, encrypted delivery, session correlation, TTL, restart interruption, and Burn invalidation. After an authored step completes, the Layer 5 Host coordinator reads the immutable attempt state and atomically claims only the next dependency-eligible authored Transfer. The claim is exact and one-use, supports multiple authored Transfer steps in one attempt, and does not assume Transform follows a PipelinePrivate landing. Layer 3 then admits transport capacity before Layer 1 moves bytes.

Transform and Execute have no worker or runtime implementation. The requester command, `BridgePlanStore::create_attempt_from_approval`, and receiver protocol each revalidate the stored immutable revision. If either primitive is present, the whole Plan is rejected before approval consumption, attempt/protocol row creation, Search/Transfer grant creation, candidate discovery, or side effects. Pastey Core does not partially execute or fake success.

## Preserved foundations

Rust retains ObjectRefs, logical identity, candidate identity, BLAKE3 integrity, approved-scope checks, Unix descriptor-oriented safe opening, Windows no-reparse/final-handle identity validation, current-session binding, Plan/revision/attempt/step correlation, one-use Search/Transfer authority, explicit PipelinePrivate Transfer, restart cleanup, TTL, and Burn. PipelinePrivate consumption reuses the same safe physical-file identity implementation instead of a weaker independent digest path.

The generic peer-capability transport remains, but current Host projections contain no concrete Transform or Execute implementation facts. A projection may validly contain zero facts; no fallback capability is fabricated. Schema/framework support is not availability.

## Non-goals

This phase does not design or implement an Agent sandbox/workspace, provider execution API, patch engine, mutation adapter, runtime adapter, Python or shell execution, process containment, network policy, or Agent tool registry.

Automated and cross-compile evidence is not physical packaged Mac↔Windows E2E proof.

The future architecture that attaches Managed Workspace, PM/Worker Agents, HostRuntime, Headless Hosts, generic object acquisition, and the separate Developer Mode authority domain to this frozen Layer 5 contract is specified in the canonical [upper architecture](../upper-architecture.md).
