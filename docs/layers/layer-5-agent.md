# Layer 5 — Managed semantic workspace

Layer 5 owns guided Plan composition, optional advisory proposals, immutable object-flow revisions, one complete requester approval, exact attempt/step authority, Host-owned continuation state, and the boundary for future Agent integration. Version 1.9.2 is the previous freeze baseline for these semantics. Version 1.9.3 adds the Phase 1–5 Host and generic managed-authority substrate without enabling the planned Phase 6 Agent product.

## Product primitives

- **Search** finds and selects an object on the reviewed Host and scopes. It is implemented.
- **Transform** records authorization to modify the selected object on its explicit current Host according to reviewed modification intent. It is framework-only.
- **Transfer** explicitly moves the current revision to another device or landing. It is implemented; `PipelinePrivate` is used only for an authored intermediate Transfer.
- **Execute** records authorization to run the exact current revision on an explicit Host according to reviewed execution intent. It is framework-only.

Only Transfer moves the object. Transform never selects another Host or creates an unrelated derivative. Execute is never hidden inside Transform or Transfer.

Developer Mode v0 is not part of Layer 5. Its human-only terminal grant cannot be created from a Plan approval or step grant, and terminal commands/results create no logical revision, primitive completion, or managed lineage. See [Developer Mode](../developer-mode.md).

## Composer and Review

The primary UX is the editable four-primitive Block Composer. Transform presents Target, Modify on, logical revision, and Modification intent. Execute presents Target, Execute on, logical revision, and Execution intent. Neither presents byte offsets, patch-worker details, interpreter controls, shell commands, arguments, cwd, environment, timeout, or process settings.

Review shows the complete object flow and clearly labels Transform/Execute as framework-defined but not currently executable. One Review & Run remains the only requester approval. The receiver has no per-step approval or run controls.

Natural-v1 uses the same vocabulary. Search/Transfer proposals may be supported; any proposal containing Transform or Execute is marked `unsupported_future` until an Agent implementation exists.

## Object flow and revision semantics

The current Search-first Composer establishes logical object `selected_file` revision 1 at its Host. Transform consumes the exact current logical revision and declares revision N+1 at the same Host. Execute consumes the exact current revision. Transfer preserves the revision while changing its location.

These revision transitions are immutable Plan dependencies. A declared post-Transform revision does not claim production bytes exist. Cross-device Transform or Execute fails validation unless an explicit prior Transfer established locality. No capability fact may insert or remove movement.

Search and `selected_file` are current v1 Composer constraints, not permanent managed-object ontology. The implemented generic binder can acquire Search results, Inbox files, drag/drop objects, local user selections, or generated artifacts as exact Host-local managed revisions without introducing a fifth primitive. Native Plan v2 can represent those generic roots, although no v2 product/UI flow invokes them yet. The contract is defined in [upper-architecture.md](../upper-architecture.md).

## Current execution boundary

Search and Transfer retain Rust-owned automatic continuation, candidate selection, encrypted delivery, session correlation, TTL, restart interruption, and Burn invalidation. After an authored step completes, the Layer 5 Host coordinator reads the immutable attempt state and atomically claims only the next dependency-eligible authored Transfer. The claim is exact and one-use, supports multiple authored Transfer steps in one attempt, and does not assume Transform follows a PipelinePrivate landing. Layer 3 then admits transport capacity before Layer 1 moves bytes.

Transform and Execute have no Worker or live product implementation. The 1.9.3 substrate can claim one eligible v2 managed step inside a crate-private Core seam, create exact process-local effect authority, and validate ordered Host evidence/result proposals. No Tauri command, Layer 4 dispatch, PM, Worker, or product coordinator calls it. The requester command, v1 store, live v2 protocol availability gate, and receiver protocol still reject a product Plan containing either primitive as a whole before execution admission or side effects. Pastey Core does not partially execute or fake success.

## Preserved foundations

Rust retains ObjectRefs, logical identity, candidate identity, BLAKE3 integrity, approved-scope checks, Unix descriptor-oriented safe opening, Windows no-reparse/final-handle identity validation, current-session binding, Plan/revision/attempt/step correlation, one-use Search/Transfer authority, explicit PipelinePrivate Transfer, restart cleanup, TTL, and Burn. PipelinePrivate consumption reuses the same safe physical-file identity implementation instead of a weaker independent digest path.

The generic peer-capability transport remains, but current Host projections contain no concrete Transform or Execute implementation facts. A projection may validly contain zero facts; no fallback capability is fabricated. Schema/framework support is not availability.

## Managed authority boundary in 1.9.3

The generic substrate includes exact `AuthorityContextV1`/`EffectEnvelopeV1`, managed runs, Host-private resource handles, overlay/output/scratch resolution, a macOS contained execution world, an independent Host-owned TCP/DNS broker, ordered evidence, and Core-only managed result validation. Linux and Windows execution worlds remain unavailable and fail closed. Tool names and task categories are not authority; network remains independently scoped and default-deny; Developer Terminal grants are type/lifecycle separated.

Version 2.0.0 is reserved for Phase 6: Worker Harness, live managed Transform/Execute coordination, PM/planner integration, v2 product/UI flow, and related Agent capability. No patch engine, document engine, Worker intelligence, or task-specific policy is implemented in 1.9.3. Headless Host remains separate future work.

Automated and cross-compile evidence is not physical packaged Mac↔Windows E2E proof.

The implemented foundation and planned Phase 6 attachment are specified in the canonical [upper architecture](../upper-architecture.md).
