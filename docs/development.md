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

Windows managed Process execution first requires the bundled Codex helper sidecars and one Host-owned setup run by the same Windows user that will run Pastey. Build with `npm run tauri:build:windows`, open an elevated PowerShell, and run the packaged product binary with:

```powershell
.\Pastey.exe --pastey-setup-windows-codex-sandbox-v1
```

The command invokes the retained upstream-derived provisioning transaction for the Codex Windows sandbox identities, credentials, ACL/capability state, Firewall/WFP state, and helper state. Pastey does not select or reinterpret those mechanics. Runtime launch never auto-elevates: absent or stale setup fails closed until the Host operator reruns this command. Restart Pastey after setup so its process-local availability result is refreshed. Source provenance and the upstream update procedure are in `src-tauri/crates/windows-codex-sandbox/UPSTREAM.md`.

Then run the opt-in native conformance test:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --test windows_execution_world -- --ignored --nocapture
```

The integration test launches the actual Pastey product binary with its private verifier. It verifies only Pastey's adopted backend contract: authorized resource read/write projection, absence of an intentionally inherited Host environment sentinel and kernel-handle sentinel, external and loopback raw-network `WSAEACCES` denial, and a termination request that reaches an observed terminal session. It does not claim the deleted Pastey restricted-principal/bootstrap/Job design. The explicit verifier CLI reports only bounded diagnostics; production availability remains fail closed behind its generic unavailable reason. A failed or unavailable probe keeps managed Process execution unavailable. This native test is the next evidence gate after automated validation; a GNU cross-build is never a substitute.

After that conformance test passes, run the opt-in native Managed Execute acceptance test from the repository root in the same non-elevated PowerShell and with the packaged Codex sidecar directory still prepended to `PATH`:

```powershell
cargo build --manifest-path .\src-tauri\Cargo.toml --features native-windows-acceptance --bin pastey --bin pastey-managed-execute-probe
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test --manifest-path .\src-tauri\Cargo.toml --features native-windows-acceptance --bin pastey managed_worker_coordinator::tests::native_windows_managed_execute_through_codex_backend -- --exact --ignored --nocapture
```

Administrator privileges are not required for this acceptance stage; use the same Windows user whose Host-owned elevated setup already completed. Success ends with `test managed_worker_coordinator::tests::native_windows_managed_execute_through_codex_backend ... ok` and a test result containing `1 passed; 0 failed`. The test binds the exact Cargo-built probe executable to the exact immutable Execute step through the Host-private process-spec seam, then uses normal admission, EffectEnvelope compilation, resource observation, managed Process enforcement, evidence, and Core finalization. It asserts bounded stdin, stdout and stderr, the Codex backend kind, the authored input revision, and absence of a new lineage revision. The feature-gated probe is not included in ordinary product builds or Tauri packaging.

On failure, preserve the complete output from both commands, the complete Stage 4 verifier output, `Get-Command codex-command-runner.exe,codex-windows-sandbox-setup.exe | Format-List *`, `$env:PATH`, `rustc -vV`, and `cargo -vV`. Do not include sandbox credentials, provider credentials, Bridge secrets, Host-private resource paths, or raw authority/evidence objects.

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
