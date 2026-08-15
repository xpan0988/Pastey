# Layer 5 — Agent-assisted device workspace

Layer 5 owns guided Plan composition, optional advisory proposals, immutable object-flow revisions, one complete requester approval, Host-owned lifecycle state, and the boundary for future Agent integration.

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

Search establishes logical object `selected_file` revision 1 at its Host. Transform consumes the exact current logical revision and declares revision N+1 at the same Host. Execute consumes the exact current revision. Transfer preserves the revision while changing its location.

These revision transitions are immutable Plan dependencies. A declared post-Transform revision does not claim production bytes exist. Cross-device Transform or Execute fails validation unless an explicit prior Transfer established locality. No capability fact may insert or remove movement.

## Current execution boundary

Search and Transfer retain Rust-owned automatic continuation, candidate selection, encrypted delivery, session correlation, TTL, restart interruption, and Burn invalidation.

Transform and Execute have no worker or runtime implementation. Attempt start checks the stored immutable revision before creating an attempt or consuming approval authority. If either primitive is present, it returns a clear Agent-not-available error. Pastey Core does not fake success.

## Preserved foundations

Rust retains ObjectRefs, logical identity, candidate identity, BLAKE3 integrity, approved-scope checks, Unix descriptor-oriented safe opening, Windows no-reparse/final-handle identity validation, current-session binding, Plan/revision/attempt/step correlation, one-use Search/Transfer authority, explicit PipelinePrivate Transfer, restart cleanup, TTL, and Burn.

The generic peer-capability transport remains, but current Host projections contain no concrete Transform or Execute implementation facts. Schema/framework support is not availability.

## Non-goals

This phase does not design or implement an Agent sandbox/workspace, provider execution API, patch engine, mutation adapter, runtime adapter, Python or shell execution, process containment, network policy, or Agent tool registry.

Automated and cross-compile evidence is not physical packaged Mac↔Windows E2E proof.
