# Development, validation, and release

Architecture belongs in [architecture](architecture.md), managed contracts in [Layer 5](layers/layer-5-agent.md), and concrete identifiers/configuration in [reference](reference.md).

## Setup and builds

```bash
npm install
npm run tauri:dev
```

Use `npm run tauri:dev-fast` only for local transfer-throughput work. Build the frontend with `npm run build`, a desktop package with `npm run tauri:build`, and a checked package with `npm run build:checked`. Linux release hosts use `npm run build:checked:linux`. Windows packages use `npm run tauri:build:windows`; that command first builds and stages the pinned Codex command-runner and setup sidecars, then supplies their `externalBin` bundle configuration to Tauri.

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

The Windows cross-check requires the GNU target and MinGW toolchain. It proves compilation, not native Windows confinement, safe-open behavior, machine setup, packaging, or physical E2E.

Native Windows acceptance is five stop-on-failure scripts run from the repository root. Each run writes a self-contained log and safe sandbox diagnostics under the gitignored `artifacts/windows-acceptance/` directory and ends with a stable `PASTEY_ACCEPTANCE_STAGE_<N>_(PASS|FAIL|BLOCKED)` token. Exit codes are `0` for PASS, `1` when the intended stage ran and failed, and `2` when required product or Host state is absent.

1. In a normal, non-elevated PowerShell, build, package, and install the current production bundle:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\scripts\windows-acceptance\stage-1-build-install.ps1
   ```

2. In an Administrator PowerShell opened under the same Windows user, run Host-owned sandbox setup:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\scripts\windows-acceptance\stage-2-elevated-setup.ps1
   ```

3. Return to a normal PowerShell and run the packaged verifier:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\scripts\windows-acceptance\stage-3-packaged-verifier.ps1
   ```

4. Run the exact ignored native Windows conformance test:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\scripts\windows-acceptance\stage-4-native-conformance.ps1
   ```

5. Build the opt-in probe and run the exact production-path Managed Execute acceptance test:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\scripts\windows-acceptance\stage-5-managed-execute.ps1
   ```

Stage 1 intentionally uses `npm ci` and the unchanged `npm run tauri:build:windows` production package/install path, so packaging failures are Stage 1 FAIL results rather than hidden workarounds. Stage 2 is the explicit elevated, Host-owned Codex sandbox setup; it never reads, copies, or prints `.sandbox-secrets/sandbox_users.json`. Stage 3 runs the installed packaged verifier. Stage 4 runs native conformance through the production backend. Stage 5 builds the opt-in Managed Execute probe and runs the exact ignored production-path Managed Execute test.

For Stage 5, the acceptance script passes the installed production `codex-command-runner.exe` path through a test-only harness. When required by Cargo's test layout, the test materializes a temporary normal sibling helper beside the actual hashed Cargo test executable. This is harness-only and does not change production helper-resolution semantics. A failed or unavailable verifier keeps managed Process execution unavailable. Source provenance and the upstream update procedure are in `src-tauri/crates/windows-codex-sandbox/UPSTREAM.md`; GNU cross-compilation is never a substitute for these native stages.

### Windows v1 managed-execution acceptance baseline

The native Windows v1 acceptance baseline has been physically demonstrated:

```text
Stage 1 PASS
Stage 2 PASS
Stage 3 PASS
Stage 4 PASS
Stage 5 PASS
```

Stage 5 proves the production Managed Execute path with an exact ManagedRevision Host read; a read-only world projection that coexists with that authorized Host read; exact executable identity; Windows Codex sandbox availability; authorized working-directory/resource projection; stdin delivery; bounded stdout/stderr capture; process exit observation; NoRawNetwork; Allowed process/resource evidence; Execute finalization with no lineage; and no unrestricted or unsandboxed fallback. This is the Windows v1 managed-execution acceptance baseline, not physical multi-Host proof.

Windows v1 follows the adopted Codex sandbox filesystem semantics. It is not claimed to prevent every read outside Pastey's projected resources; OS/platform-default readable areas may remain readable but mint no Pastey authority. `AuthorityNeutralEnvironment` is not a literally empty environment. `CancellableProcessSession` is weaker than the old strict NoDaemonSurvival contract, and the adopted backend does not claim live descendant CPU/RSS accounting. Codex sandbox setup or credential lifecycle can become stale or incompatible, so runtime availability must fail closed. If native acceptance reports stale or missing sandbox credentials, or Win32 logon error `1326`, rerun Stage 2 elevated setup and then resume from the failing native stage. Never inspect `sandbox_users.json` to diagnose or recover that state.

Windows v1 bring-up is closed. Do not proactively refactor or harden Codex-derived Windows internals. Future Windows work should be triggered only by a concrete product regression, an authority/fail-closed violation, an upstream Codex update, or a required product capability. Primary new ExecutionWorld engineering focus can move to macOS/Linux and the remaining 2.0 product surface.

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
- B and C report a verified execution world for those specifications (macOS after its local probe; Windows only after its native product-binary probe; Linux fails closed);
- the frontend or an approved test driver exposes compose/review/approve/start/status/cancel without bypassing the registered Tauri commands;
- every authored step runs on a remote receiver Host; requester-local self-admission/execution is unavailable;
- three packaged instances have distinct HostRefs, one active Bridge, current unambiguous routes, and the exact managed root already bound at B.

The repository does not currently satisfy the first three product-surface requirements end to end: provider configuration/health and process binding remain Host-private seams. The 2.0 UI can operate the authoritative lifecycle for an existing revision, but Draft origination and the detailed topology/result projections required for this smoke are not renderer-exposed. The backend contracts can be tested, but the A→B→C physical managed smoke is therefore **not yet runnable as a normal product flow**.

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

With packaged builds on two supported desktops, record the logical Bridge id and current exact peer-session ids where diagnostics expose them, then verify:

1. Connect A/B, Quit B normally, and confirm A leaves healthy Connected state. Restart B and confirm both retain the same logical Bridge, establish fresh exact sessions, converge on one member each, show no duplicate peer, and reject every old route.
2. Open New Bridge and confirm no Bridge is created. Exercise Nearby request/remote Accept and manual 8-digit join separately; each successful action creates or joins exactly one Bridge. Confirm Devices remains inspection-only and cannot create a Bridge.
3. From A's selected Bridge, switch the central workspace to Developer Mode and request B. Confirm B sees Accept/Deny without first opening Developer Mode. Accept once, run a harmless command, inspect output, and End session. Request again, Deny, and confirm A receives a terminal denied state and B starts no PTY.
4. Navigate Bridge → Inbox → Devices → Settings → Bridge → Developer Mode → Bridge while transferring a few items and disconnecting/reconnecting. Confirm one selected Bridge/context, no duplicate listeners, members, items, or requests, no stale send target, and no lifecycle mutation from navigation.
5. Burn locally and confirm immediate removal from every local renderer surface, rejection of late events, and no claim that the remote Host also burned.

Also exercise Search/Transfer Review & Run, explicit departure, and ordinary text/file transfer. Verify paired-device display identity neither auto-joins nor authorizes a capability. Record failures; do not infer packaged or physical PASS from automated tests.

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
