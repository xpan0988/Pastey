# Development, validation, and release

Architecture belongs in [architecture](architecture.md), managed contracts in [Layer 5](layers/layer-5-agent.md), and concrete identifiers/configuration in [reference](reference.md).

## Setup and builds

```bash
npm install
npm run tauri:dev
```

Use `npm run tauri:dev-fast` only for local transfer-throughput work. Build the frontend with `npm run build`, a desktop package with `npm run tauri:build`, and a checked package with `npm run build:checked`. Linux release hosts use `npm run build:checked:linux`.

## Validation stack

Run:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml --bin pastey
cargo check --manifest-path src-tauri/Cargo.toml --bin pastey --profile dev-fast
cargo test --manifest-path src-tauri/Cargo.toml -- --nocapture
cargo check --manifest-path src-tauri/Cargo.toml --tests --target x86_64-pc-windows-gnu
npm run build
node scripts/run-natural-v1-tests.mjs
node scripts/run-natural-v2-tests.mjs
node scripts/run-layer4-validation-matrix.mjs
node scripts/run-transfer-planner-tests.mjs
npm run check:version
git diff --check
```

The Windows cross-check requires the GNU target and MinGW toolchain. It proves compilation, not native Windows confinement, safe-open behavior, packaging, or physical E2E.

## Transfer and Layer 4 validation

Transfer planner scenarios can also be replayed with:

```bash
node scripts/replay-transfer-planner-scenarios.mjs
```

The generated transfer fixture corpus is documented in [tests/fixtures/transfer-corpus/README.md](../tests/fixtures/transfer-corpus/README.md). Generated payloads are local-only and must not be committed.

For a single-machine dual-instance smoke, create/join a Bridge and exercise selected-peer ordinary data, Room Control, Search/Transfer Review & Run, disconnect/reconnect, and Burn. For a two-device smoke, repeat with packaged builds on independent LAN Hosts and record the evidence as described below.

## Automated, local, and physical evidence

- Rust and TypeScript tests cover deterministic composition, alias resolution, immutable correlation, whole-Plan readiness, replay, Worker/provider streaming, effect enforcement, Core results, exact Transfer receipt, cancellation/revocation races, restart, and Burn.
- A local dual-instance run covers desktop wiring, current-session Room Control, ordinary Transfer, and protocol interaction on one machine. It does not prove independent physical Hosts, LAN failure behavior, native Windows, or a verified managed process world on another platform.
- Physical multi-device proof requires packaged builds on the named devices and a recorded run of the procedure below. Do not report PASS from unit tests, Darwin-only integration, cross-compilation, or source inspection.

## Phase 6 physical multi-Host smoke

The target scenario is:

```text
A requester → Transform N→N+1 @ B → authored Transfer B→C → Execute N+1 @ C
```

### Current readiness gate

Do not start or claim this smoke as reproducible until all of these are true:

- the product can create/select a durable provider configuration on B and C and run the no-effect health probe;
- the product can bind the exact reviewed Transform/Execute steps to Host-owned process specifications where Process is required;
- B and C report a verified execution world for those specifications (currently macOS only; Linux/Windows fail closed);
- the frontend or an approved test driver exposes compose/review/approve/start/status/cancel without bypassing the registered Tauri commands;
- every authored step runs on a remote receiver Host; requester-local self-admission/execution is unavailable;
- three packaged instances have distinct HostRefs, one active Bridge, current unambiguous routes, and the exact managed root already bound at B.

The repository does not currently satisfy the first three product-surface requirements end to end: provider configuration/health and process binding are Host-private seams, and no full Agent lifecycle UI exists. The backend contracts can be tested, but the A→B→C physical managed smoke is therefore **not yet runnable as a normal product flow**.

### Procedure once the gate is implemented

Record app version/build id, OS/architecture, HostRef, Bridge id, provider id/generation/model (never the credential), execution-world probe result, and wall-clock time for A/B/C.

1. On B, bind a safe test object as managed revision N. Configure and health-check the provider on B and C. Verify the health operation creates no Plan, Room Control event, grant, or effect.
2. On A, compose a native-v2 Plan with exactly three steps: Transform N→N+1 at B; Transfer N+1 B→C dependent on Transform; Execute N+1 at C dependent on Transfer. Review the displayed Hosts, topology, movement, object/revisions, intents, and hash.
3. Approve once and start one attempt. Confirm all Hosts report ready before any Search, Worker effect, or Transfer begins. Confirm B/C prepare exact admissions before A commits.
4. Confirm B alone runs Transform. Capture bounded product/Worker status and verify Core records N+1 at B. Before Transfer receipt, assert C cannot resolve N+1 and Execute remains pending.
5. Confirm only the authored Transfer uses the normal encrypted transfer path. On C, verify the receipt matches attempt, step, revision id/hash, logical object/revision, content digest, destination HostRef, and current binding.
6. Confirm A accepts the Transfer result and broadcasts the exact step commit. Only then confirm Execute becomes eligible and runs on C. Verify Execute records a result digest and creates no N+2 or other lineage.
7. Confirm final status on A/B/C names the same attempt/revision/hash and exact completed step set. Verify no implicit Host change, hidden Transfer, Worker NetworkGrant, or Developer Terminal session exists.

### Failure matrix

Use a fresh approved revision/attempt for each case; terminal attempts are never resumed.

- Cancel before provider call, during provider streaming, during Process, during Transfer, and after Worker proposal but before Core completion. Assert no late success or dependent step.
- Disconnect or replace B/C session during readiness, prepared state, Worker execution, Transfer, and step-commit delivery. Assert old binding rejection and new admission on any later fresh attempt.
- Revoke/delete the exact provider generation before start and during a run. Assert no provider substitution and no authority widening.
- Burn B or C during readiness, Worker execution, and Transfer. Assert local cleanup, run/world termination, no late receipt/result, and no remote Burn shortcut.
- Restart A/B/C in checking-readiness, prepared, running, and post-Transfer/pre-commit states. Assert process-local Worker/grant/world state is not restored and durable attempts become interrupted.
- Replay review, start, commit, Worker dispatch, result, receipt, and cancellation messages. Assert duplicate/late rejection.
- Drop coordination delivery after local Core completion. Record the resulting interrupted/stale presentation behavior; this remains a recovery/UI acceptance test, not a reason to treat an uncommitted result as global success.

Retain non-secret logs/status exports, revision/hash, receipt metadata, and screenshots as manual evidence. Never record provider credentials, Host paths, ObjectRefs, grants, raw evidence internals, or terminal contents.

## Ordinary two-device smoke

With packaged builds on two supported desktops, create/join a Bridge, exercise nearby/code join, ordinary text/file transfer, Search/Transfer Review & Run, disconnect/reconnect route expiry, explicit departure, and Burn. Verify paired-device display identity neither auto-joins nor authorizes a capability.

Native packaged Windows must separately exercise normal-file Search/Transfer, reparse/path substitution rejection, identity/digest checks, explicit movement, restart, and Burn. Native packaged macOS must exercise the descriptor-oriented no-follow and identity path.

## Developer Terminal physical checks

On macOS and Windows in both controller/Host directions, verify prompt/VT rendering, focus/cursor, Unicode, `cd`/location/list commands, Backspace/Delete/arrows/Home/End/Tab/Ctrl+C/Ctrl+D/Ctrl+L, resize, long output, explicit close, disconnect, reconnect requiring new admission, Burn, and transfer contention. Stress rapid typing, held keys, Backspace, bounded paste, and cancellation while output is active. This checklist remains unclaimed until performed on physical devices.

## Release

`src-tauri/Cargo.toml` is the authoritative packaged app version. Release with:

```bash
npm run release:version -- X.Y.Z "Release Title"
git push origin main --tags
```

The script requires a greater unused version, updates derived version files and release documentation, runs its checks, stages only release-file edits, creates `chore(release): vX.Y.Z`, and creates annotated tag `vX.Y.Z`. Use `--dry-run` to preview. It refuses a dirty worktree unless `--allow-dirty` is explicit and never pushes automatically.

Its built-in checks are Cargo formatting, Cargo check, and version consistency. A release pass must additionally run the full validation stack and packaged physical smoke appropriate to the release claim.

## Repository hygiene

Preserve unrelated worktree changes. Review `git status --short`, `git diff --check`, and the exact staged diff. Do not commit generated fixtures, build output, credentials, or temporary audit notes. Use ProGraph for navigation, then source/compiler/tests for authority. Update the canonical document that owns a topic instead of adding another status narrative; keep release history in [CHANGELOG](../CHANGELOG.md).
