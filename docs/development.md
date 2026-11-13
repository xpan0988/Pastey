# Development, validation, and release

This document describes how the current cross-device Agent execution substrate is built and validated. Architecture belongs in [architecture](architecture.md), native and managed contracts in [Layer 5](layers/layer-5-agent.md), Windows managed-backend semantics in [Windows managed execution](platform/windows-managed-execution.md), and concrete identifiers/configuration in [reference](reference.md).

## Setup and builds

```bash
npm install
npm run tauri:dev
```

Use `npm run tauri:dev-fast` only for local transfer-throughput work. Build the frontend with `npm run build`, a desktop package with `npm run tauri:build`, and a checked package with `npm run build:checked`. Linux release hosts use `npm run build:checked:linux`. Windows release hosts use `npm run build:checked:windows`; it checks the version, builds and stages the pinned Codex command-runner and setup sidecars through `tauri:build:windows`, applies their `externalBin` bundle configuration, builds the installers, and audits the bundle. Windows SemVer prereleases build NSIS; stable versions build NSIS and MSI.

## Validation stack

Run:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml --bin pastey
cargo check --manifest-path src-tauri/Cargo.toml --bin pastey --profile dev-fast
cargo test --manifest-path src-tauri/Cargo.toml -- --nocapture
cargo check --manifest-path src-tauri/Cargo.toml --tests --target x86_64-pc-windows-gnu
npm run build
npm run test:frontend-integration
node scripts/run-natural-v1-tests.mjs
node scripts/run-natural-v2-tests.mjs
node scripts/run-layer4-validation-matrix.mjs
node scripts/run-transfer-planner-tests.mjs
npm run check:version
git diff --check
```

Some Rust tests bind local TCP ports and open a PTY; a sandboxed shell must allow both or those tests fail with `Operation not permitted`. The Windows cross-check requires the GNU target and MinGW toolchain. It proves compilation, not native Windows confinement, safe-open behavior, machine setup, packaging, or physical E2E.

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

Stage 1 intentionally uses `npm ci` and the `npm run tauri:build:windows` production package/install path, so packaging failures are Stage 1 FAIL results rather than hidden workarounds. Stage 2 is the explicit elevated, Host-owned Codex sandbox setup; it never reads, copies, or prints `.sandbox-secrets/sandbox_users.json`. Stage 3 runs the installed packaged verifier. Stage 4 runs native conformance through the production backend. Stage 5 builds the opt-in Managed Execute probe and runs the exact ignored production-path Managed Execute test.

A failed or unavailable verifier keeps managed Process execution unavailable. Windows semantics and limitations are in [Windows managed execution](platform/windows-managed-execution.md); source provenance and the upstream update procedure are in [`UPSTREAM.md`](../src-tauri/crates/windows-codex-sandbox/UPSTREAM.md). GNU cross-compilation is never a substitute for these native stages.

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

If native acceptance reports stale or missing sandbox credentials, or Win32 logon error `1326`, rerun Stage 2 elevated setup and then resume from the failing native stage. Never inspect `sandbox_users.json` to diagnose or recover that state.

Windows v1 Stage 1–5 acceptance is complete. Future Windows work should be triggered by a product regression, an authority or fail-closed violation, an upstream Codex update, or a required product capability rather than speculative sandbox refactoring.

## Transfer and Layer 4 validation

Transfer planner scenarios can also be replayed with:

```bash
node scripts/replay-transfer-planner-scenarios.mjs
```

The generated transfer fixture corpus is documented in [tests/fixtures/transfer-corpus/README.md](../tests/fixtures/transfer-corpus/README.md). Generated payloads are local-only and must not be committed.

For a single-machine dual-instance smoke, create/join a Bridge and exercise selected-peer ordinary data, Room Control, Search/Transfer Review & Run, disconnect/reconnect, and Burn.

The internet carrier's contract ([Connectivity](layers/layer-4-connectivity.md)) is tested in `connectivity::tests`: two real Hosts from the Room Control pair harness (production room routers on loopback) joined by the in-memory carrier, covering card validation and pinning, per-Bridge stream admission, reconnect and `/join` through the tunnel, a session surviving a carrier outage and a path change, a continuation running once across an outage, and revocation by Burn, forgetting a device and re-pairing; plus one test of real iroh QUIC on loopback with relays off (unpinned key refused, pinned key carries streams both ways). They bind loopback sockets:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey connectivity::
```

These automated checks do not exercise NAT traversal, relays, real networks or Windows. For a physical regression check, pair the devices on the LAN, enable Across the internet on both, then put them on separate networks. Check the reported path, reconnect, run Codex remotely and verify the outcome after an outage without resending the turn. Use [net-probe](../tools/net-probe/README.md) to isolate the raw path from the Bridge.

The pairing and authenticated transfer regression suites also use loopback sockets:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey -- bridge_auth join_authentication discovery:: bridge_lifecycle:: storage::tests::a_pairing_code storage::tests::a_pinned_peer commands::tests::a_code_pairs
```

## Automated, local, and physical evidence

- Rust and TypeScript tests cover deterministic composition, alias resolution, immutable correlation, whole-Plan readiness, replay, Worker/provider streaming, effect enforcement, Core results, exact Transfer receipt, cancellation/revocation races, restart, and Burn.
- A local dual-instance run covers desktop wiring, current-session Room Control, ordinary Transfer, and protocol interaction on one machine. It does not prove independent physical Hosts, LAN failure behavior, native Windows, or a verified managed process world on another platform.
- Source-level Native Agent reliability and deterministic two-Host state validation are complete. The test-only pair harness uses two independent durable Host states; it does not exercise physical machines.
- **2.0.0 release evidence.** The release owner reported packaged Mac ↔ Windows runs covering a Bridge across separate networks (home Wi-Fi and phone hotspot), remote Windows Codex from the Mac, Host identity and the secure session, unattended Full Access, connection and Agent recovery, and Developer Mode recovery. The per-step records are not in the repository. A separate Mac → Windows run found and inspected a Codex Desktop thread, refused to continue it while held, and continued it in place once the Codex clients were closed.
- **Subsequent physical acceptance.** The maintainer confirmed end-to-end Native Workspace Movement, result Return and Apply, and Full Access while Windows is locked. There is no per-case record here to establish specific builds, directions, timings or failure-matrix results for those runs. Relay-only operation, sleep/wake and the full Generic Managed multi-Host path are outside these recorded results.
- **Local Claude Code Conversation.** On macOS, signed-in Claude Code 2.1.317 passed new and follow-up turns, Stop, and cleanup after Pastey was killed, through the production start method and the built lifeline. The tests did not drive the packaged UI or IPC. A separate headless probe exercised interruption and cleanup with a real tool call. On Windows 11 with 2.1.319, signed-in adapter runs covered a new conversation and its follow-up, Stop with a real tool process, and process cleanup after Pastey was killed. A debug app run also exercised the local flow and untrusted-workspace refusal through the real WebView and IPC; the folder picker and interactive trust prompt were not driven there. A paired Mac's view of a Windows Host with Claude Code enabled was not run in that record; local-only advertisement and admission are covered by the automated checks and Core's no-peer-protocol rule.
- **Remote Claude Code (Mac → Windows).** Automated: the two-Host Room Control pair test (`windows_claude_code_runs_and_continues_for_the_mac_only_after_accept`, real room router on loopback, fixture Claude Code on the "Windows" Host) covers a new conversation and an existing-session continuation in a shared folder, each only after Accept and run once, Deny, a held session refused with nothing sent, and an unshared reference refused before Review; unit tests cover Full Access admission, share switches, marker, held/rival/concurrent-writer cases and option validation. Live on macOS with signed-in Claude Code 2.1.318 through the production lifecycle and lifeline: an existing session Pastey did not create was continued in place with `--model haiku --effort low` (same session file, earlier context recalled, discovery then read `claude-haiku-5-5` / `low` where it read `claude-opus-5-5` / `medium` before), and a session held idle by another headless run was refused naming the holder with nothing written. Not yet run: any of this on Windows, signed in, and the packaged Mac ↔ Windows flow (see [Remote Claude Code physical acceptance](#remote-claude-code-physical-acceptance)).
- **Windows Codex Stop.** Merged in `8b7288e`. The Windows fixture suite passed, and a signed-in adapter test with Codex 0.163.0 confirmed the sandboxed command was gone when Stop was reported and the next turn resumed the conversation. That record does not cover the packaged app, the bundled desktop Codex or the restricted-token sandbox.

## Native Agent focused validation

The Native Agent path has focused Rust coverage in `native_agent`, `commands`, `room_control`, `transfer`, `storage`, and `host_runtime`. Run the available module tests with:

```bash
cargo test --manifest-path src-tauri/Cargo.toml native_agent -- --nocapture
```

These tests cover Host-native session reuse and terminal-success handling, capability-neutral adapter dispatch and opaque payload handling (including a test-only `fake.longjob.v1` adapter), immutable Local/Bridge task ownership and Bridge-bound cancellation, executor-side admission and one-shot Review for direct invocation and received workspaces, authenticated remote task correlation and cancellation, requester-reviewed workspace movement, transfer metadata validation and final-acknowledgement ambiguity, source ownership and revalidation, restart-safe recovery projection, monotonic lifecycle/replay handling, Bridge Burn authority purge, durable conflict retention, and exact current-session Room Control handling. `npm run test:frontend-integration` additionally checks that unresolved movement blocks a new run while Reconcile, Stop, and result-Return repair stay reachable, and that the executor Review banner offers only Accept or Deny over a projection without invocation input. They are local automated evidence, not physical multi-device proof.

Live tests against the Codex installed on this machine are ignored by default, because they need an installed, signed-in Codex and use the account's model quota. On a Mac with only the ChatGPT desktop app, they use its bundled Codex:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_codex -- --ignored --nocapture
```

- `installed_codex_is_discovered_with_its_backend` reports which installation Pastey selects.
- `installed_codex_runs_a_bridge_task_with_its_requesters_approval` runs a Bridge task from a verified requesting device under a read-only sandbox, so the write needs that requester's native approval through `submit_native_decision`.
- `installed_codex_modifies_a_disposable_original_workspace_and_reuses_its_session` runs two local turns in one session. It runs with `approval_policy="never"` and `sandbox_mode="workspace-write"`, because a local task's native approvals cannot be answered from Pastey.

The last two pass their settings through Codex's own `-c` flag for that run only; the user's configuration is unchanged. They work in disposable folders.

Read-only session discovery has a live check against this Host's real Claude Code and Codex sessions. It defaults to this checkout as the workspace; set `PASTEY_SESSION_WORKSPACE` to another folder. It starts, resumes and changes nothing, and prints statuses and evidence with titles reduced to their length:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_sessions_are_discovered_and_inspected_read_only -- --ignored --nocapture
```

Remote session reading has a deterministic two-Host check without sockets. In it, the Mac's query passes the Windows Host's Room Control validator, Windows resolves the Mac's permission from its own storage and answers from a real Claude Code store, and the answer passes the Mac's validator into its outstanding query. Run it with `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey native_session_pair_tests`. It does not exercise the HTTP transport, a running app, or Windows.

Full Access has unit tests (`full_access`, `session_read_access`, the `full_access_*` and landing tests in `native_agent`, the Developer Terminal tests) and a two-Host check over the real room router and Room Control: the Mac opens a Windows folder by path and continues a Codex thread with no Review on Windows, a terminal opens without Accept, and revoking ends both. The terminal tests need a PTY, which a sandboxed shell may refuse:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey full_access
```

The automated pair uses neither a packaged app nor a native Windows session. For physical use and regression setup, see [Unattended Windows readiness](reference.md#unattended-windows-readiness).

### Timing-sensitive tests

If the Room Control burst test fails on its query/answer counts, rerun it alone on an idle machine. Its shared projection can unblock callers before the final answer reaches the requester, so a count mismatch at that instant does not by itself prove loss or replay.

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey -- --exact room_control::runtime_pair_tests::a_burst_beyond_the_budget_is_refused_cleanly_and_recovers
```

### Claude Code live validation

The separate Claude Code background-session continuation has a live check on macOS. It uses the installed, signed-in Claude Code and a little of its quota. In `src-tauri/target/claude-s2-live` it creates a fresh session, continues it through Pastey's Native Agent lifecycle, and then shows that a session a foreground run holds is refused without anything being sent:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_claude_code_continues_an_unheld_session_and_refuses_a_held_one -- --ignored --nocapture
```

Two more live checks kill a separate Pastey process (this test binary) with SIGKILL while Claude Code runs a continuation. They then reopen its database:
- the task must read "outcome unknown" while it holds its folder and session, and then be observed to its real end;
- or, when stopped, it must be confirmed through Claude Code's registry.

Run them one at a time, from a clean environment: a shell inside a Claude Code session passes its host variables to every child `claude`.

```bash
env -i HOME="$HOME" USER="$USER" PATH="$PATH" TMPDIR="$TMPDIR" LANG=en_US.UTF-8 cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_claude_code_ -- --ignored --nocapture --test-threads=1
```

That filter also selects the Claude Code Conversation checks and the headless existing-session check (an existing session continued in place with a chosen model and effort, then a held session refused). Before running it, build the real lifeline with `cargo build --manifest-path src-tauri/Cargo.toml --bin pastey`. The Conversation checks exercise:
- a conversation and a follow-up that remembers it;
- Stop during a turn;
- a Pastey process killed with SIGKILL during a turn: Claude Code must end without anyone stopping it, and the restored task must never read failed.

They use disposable folders under `src-tauri/target/`. Open the checkout in Claude Code and grant its own workspace trust before running tests that write project settings.

On Windows, the Claude Code conversation adapter has its own suites. They run real Windows processes in the product's Job Object (a fixture `claude.cmd` with PowerShell `-p` runs, `ping`, `conhost`), and no Claude Code:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey -- claude_code::job:: conversation::windows_tests --test-threads=4
```

For the Windows live checks, use a fresh PowerShell outside any Claude Code session. Install the standalone CLI (2.1.319 or later), sign in, and check `claude --version` and `claude auth status`. Run from a disposable checkout trusted by Claude Code. The live checks use the account's model quota. The Stop and crash tests grant only `Bash(ping:*)` in their disposable folders' own Claude Code settings so they can observe a real tool process.

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_claude_code_conversation_ -- --ignored --nocapture --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey windows_stop_tests
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_codex_stop_ends_the_command_the_turn_ran -- --ignored --nocapture
```

The last two commands check Windows Codex Stop. The ignored Codex test requires a signed-in Codex installation with its Windows sandbox set up. For isolated diagnosis use the [Codex Stop probe](../tools/codex-stop-probe/README.md).

### Remote Claude Code physical acceptance

Not yet recorded. Run on the Windows Host first, in a fresh PowerShell outside any Claude Code session, signed in with 2.1.319 or later, from a checkout Claude Code trusts:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey -- claude_code::job:: conversation::windows_tests --test-threads=4
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_claude_code_conversation_ -- --ignored --nocapture --test-threads=1
```

The headless existing-session check (`installed_claude_code_continues_an_existing_session_in_place_with_a_chosen_model`) uses the Unix lifeline and runs on macOS; on Windows the same path is exercised by the packaged flow below. Then, packaged, Mac (no Claude Code installed) and Windows paired:

1. Windows: Bridge details → Workspaces → share a disposable, Claude-trusted folder with Sessions and Agent requests on.
2. Mac: Agents → Windows → that folder → New conversation → Claude Code; send a prompt. Windows: Accept. Expect one run, the reply on the Mac, and the session listed under that folder.
3. Mac: open an existing session there (one made with `claude` on Windows, then closed); expand "Model and effort", choose a model alias and `low`; send. Windows: Accept. Expect the same session id afterwards (Details → Ids), its context used, and the session strip showing the chosen model and `low`.
4. Open that session in the Claude desktop app or `claude --resume` on Windows, leave it idle; continue it from the Mac. Expect "An interactive Claude Code session … has this session open", nothing written.
5. Deny one request; grant Full Access and repeat step 2 without Accept; revoke and check the next request waits again.
6. Stop a long turn from the Mac (expect "Stopped" only once the Job Object is empty); disconnect the network during a turn and reconnect (expect Reconcile, no second turn); Burn the Bridge during a turn.
7. Kill Pastey on Windows during a turn and restart it: expect "outcome unknown", then the observed end; nothing re-sent.

Record versions, steps and outcomes only; omit session ids, paths and transcript content.

### Native Workspace Movement regression checks

The test-only `native_agent::tests::pair_harness` scenarios use Requester Host A and Executor Host B with separate `AppPaths`, SQLite databases, and `NativeAgentServiceV1` instances, each independently restarted from durable state. Two compositions share it. The Codex workspace-movement pair uses a deterministic fake Codex app-server that writes app-server launches and native turns to external ledgers. The capability pair registers only the test-only `fake.longjob.v1` adapter, with Codex absent, to prove the lifecycle is capability-neutral. Executor admission scenarios in both show that a direct remote invocation or a landed workspace waits for executor Review without any adapter `prepare`/`start` or app-server launch. The fixture controls delivery and fault timing while exercising production state, movement, regular-file-set package/materialization, task, reconciliation, Return, apply, cancellation, and Burn code; it does not implement another Native Agent state machine. Run it with `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey native_agent::tests::pair_harness`. It gives deterministic evidence for authority, task ownership, executor admission and Review, movement, restart, reconciliation, exact Return and retry, conflict retention, apply idempotence, cancellation, Burn, late completion/observer behavior, and Agent-at-most-once. It does not prove APFS ↔ NTFS behavior, real macOS/Windows process lifecycle, installed and authenticated Codex, real app-server behavior, LAN/socket interruption and reconnect, sleep/wake, firewall/platform behavior, or packaged release UX. Use physical hosts to exercise those boundaries.

For a packaged regression run on two devices, use disposable folders. Verify secure pairing and reconnect, requester movement approval, executor Accept and Deny, encrypted outbound transfer, one Codex turn, exact result Return and Apply, and conflict retention when the source changes. Exercise a network drop or restart and verify recovery does not run a second turn. Verify that Burn cuts off Bridge authority. To check Full Access while locked, grant it on Windows, lock with Win+L while Pastey is running, and from the Mac start Codex and open a Developer Terminal without an Accept on Windows. Verify revocation prevents later requests from starting without Accept. Record only non-secret evidence; omit credentials, native session ids, private paths, grants, and raw Agent or terminal content.

## Windows crash diagnosis

Check the Application event log for `pastey.exe` (Application Error 1000 or Windows Error Reporting 1001), and note the faulting module and exception code. For WebView crashes look for `msedgewebview2.exe`. The app log records bounded panic details, unclean exit on next launch and Native Agent steps by task id. Do not share WER dumps.

```powershell
Get-WinEvent -FilterHashtable @{LogName='Application'; Id=1000,1001; StartTime=(Get-Date).AddDays(-7)} | Where-Object { $_.Message -match 'pastey|msedgewebview2' }
Get-Content "$env:LOCALAPPDATA\pastey\logs\pastey.log" -Tail 300
```

## Developer Terminal physical checks

On macOS and Windows in both controller/Host directions, verify prompt/VT rendering, focus/cursor, Unicode, `cd`/location/list commands, Backspace/Delete/arrows/Home/End/Tab/Ctrl+C/Ctrl+D/Ctrl+L, resize, long output, explicit close, disconnect, reconnect requiring new admission, Burn, and transfer contention. Stress rapid typing, held keys, Backspace, bounded paste, and cancellation while output is active. This checklist remains unclaimed until performed on physical devices.

## Release

`src-tauri/Cargo.toml` is the authoritative packaged app version. Release with:

```bash
npm run release:version -- 2.0.0-beta.3 "Pastey 2.0 Beta 3" --dry-run
npm run release:version -- 2.0.0-beta.3 "Pastey 2.0 Beta 3"
git push origin main --tags
```

Stable versions use the same command, for example `npm run release:version -- 2.0.0 "Pastey 2.0" --dry-run` followed by the command without `--dry-run` and the tag push. The script requires an unused version with greater SemVer precedence than the packaged version. Prereleases sort below the corresponding stable version; build metadata does not change precedence. It rejects malformed versions before editing files and refuses a dirty worktree unless `--allow-dirty` is explicit.

The command updates `src-tauri/Cargo.toml`, `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.lock` to the exact version. It archives the current `## Unreleased` CHANGELOG body under the new version and date, leaving a clean section for later beta fixes. When `docs/release-notes/` exists, it also writes a short note for that version. It runs Cargo formatting, Cargo check, and `npm run check:version`, stages only release files, commits `chore(release): v<version>`, and creates annotated tag `v<version>`. The dry run previews these actions without writes; the command never pushes automatically.

Focused release-tool checks run with `node --test tests/releaseTooling.test.mjs`; they use temporary fixtures for prerelease version synchronization and do not change the checkout or create tags.

Pushing the tag triggers GitHub Actions installer builds for macOS Apple Silicon, Windows, and Linux x86_64. A SemVer prerelease tag produces a GitHub Pre-release and is not marked latest; a stable tag produces a normal release, published as soon as the first job uploads. When `.github/release-notes/<tag>.md` exists in the tagged commit, it becomes the release body; otherwise the body is empty. Windows prereleases require the sidecar-aware NSIS installer and do not require MSI; stable Windows releases require both NSIS and MSI. Artifact normalization requires the tag, packaged version, source artifact, and output filename to agree on the full version, including any prerelease suffix. The builds are not code-signed or notarized.

Release history of the pipeline: beta.1 uploaded only its macOS DMG; beta.2 uploaded the DMG and the Windows NSIS installer. The Linux job has failed in its Rust check or test step on every release run since 1.9.1, so no Linux package has been published since then. 2.0.0 is the first release to build the Windows MSI with the Codex sandbox sidecars. Before pushing that tag, push the release commit alone (`git push origin main`), run `npm run build:checked:windows` from it on Windows to see both installers build, and only then push the tag (`git push origin v2.0.0`).

Run the validation stack and packaged physical smoke appropriate to the release claim. Report the checks actually run for that release separately from earlier results.

## Repository hygiene

Preserve unrelated worktree changes. Review `git status --short`, `git diff --check`, and the exact staged diff. Do not commit generated fixtures, build output, credentials, or temporary audit notes. Use ProGraph for navigation, then source/compiler/tests for authority. Update the canonical document that owns a topic instead of adding another status narrative; keep release history in [CHANGELOG](../CHANGELOG.md).
