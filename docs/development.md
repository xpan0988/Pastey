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

## Automated, local, and physical evidence

- Rust and TypeScript tests cover deterministic composition, alias resolution, immutable correlation, whole-Plan readiness, replay, Worker/provider streaming, effect enforcement, Core results, exact Transfer receipt, cancellation/revocation races, restart, and Burn.
- A local dual-instance run covers desktop wiring, current-session Room Control, ordinary Transfer, and protocol interaction on one machine. It does not prove independent physical Hosts, LAN failure behavior, native Windows, or a verified managed process world on another platform.
- Source-level Native Agent reliability and deterministic two-Host state validation are complete. The test-only pair harness uses two independent durable Host states; it does not exercise physical machines.
- Physical Native Agent proof still requires packaged builds on a Mac and a Windows Host and recorded acceptance evidence. The deterministic pair harness does not replace this pending procedure.

## Native Agent focused validation

The Native Agent path has focused Rust coverage in `native_agent`, `commands`, `room_control`, `transfer`, `storage`, and `host_runtime`. Run the available module tests with:

```bash
cargo test --manifest-path src-tauri/Cargo.toml native_agent -- --nocapture
```

These tests cover Host-native session reuse and terminal-success handling, authenticated remote task correlation and cancellation, one-review workspace movement, transfer metadata validation and final-acknowledgement ambiguity, source ownership and revalidation, restart-safe recovery projection, monotonic lifecycle/replay handling, Bridge Burn authority purge, durable conflict retention, and exact current-session Room Control handling. `npm run test:frontend-integration` additionally checks that unresolved movement blocks a new run while Reconcile, Stop, and result-Return repair stay reachable. They are local automated evidence, not physical multi-device proof.

The test-only `native_agent::tests::pair_harness` scenarios use Requester Host A and Executor Host B with separate `AppPaths`, SQLite databases, and `NativeAgentServiceV1` instances, each independently restarted from durable state. A deterministic fake native Agent writes to an external turn ledger. The fixture controls delivery and fault timing while exercising production state, movement, regular-file-set package/materialization, task, reconciliation, Return, apply, cancellation, and Burn code; it does not implement another Native Agent state machine. Run it with `cargo test --manifest-path src-tauri/Cargo.toml --bin pastey native_agent::tests::pair_harness`. It gives deterministic evidence for authority, movement, restart, reconciliation, exact Return and retry, conflict retention, apply idempotence, cancellation, Burn, late completion/observer behavior, and Agent-at-most-once. It does not prove APFS ↔ NTFS behavior, real macOS/Windows process lifecycle, installed and authenticated Codex, real app-server behavior, LAN/socket interruption and reconnect, sleep/wake, firewall/platform behavior, or packaged release UX. Those boundaries still require physical Mac ↔ Windows acceptance.

The remaining Phase 2 gate is a Native Agent physical multi-device procedure and recorded Mac ↔ Windows evidence before claiming movement reliability across actual devices. It must exercise a connected remote Host, explicit movement review, encrypted outbound and return transfer, stale/replaced-session rejection, interruption and restart behavior, cancellation races, replay rejection, source-change conflict retention, and recovery presentation. Keep recorded evidence non-secret: never include provider credentials, native session identifiers, Host paths, grants, ObjectRefs, or raw Agent/terminal content.

## Manual two-device smoke

With packaged builds on two supported desktops, a manual Bridge smoke may exercise ordinary transfer and lifecycle behavior: connect two Hosts, restart one Host and verify fresh sessions reject old routes, exercise ordinary text/file transfer and explicit departure, and Burn locally without claiming a remote Burn. This is not a replacement for Native Agent physical validation.

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

Pushing the tag triggers GitHub Actions installer builds for macOS Apple Silicon, Windows, and Linux x86_64. A SemVer prerelease tag produces a GitHub Pre-release and is not marked latest; a stable tag produces a normal release. Windows prereleases require the sidecar-aware NSIS installer and do not require MSI; stable Windows releases require both NSIS and MSI. Artifact normalization requires the tag, packaged version, source artifact, and output filename to agree on the full version, including any prerelease suffix. The existing beta.1 release remains an incomplete pipeline probe: its macOS DMG uploaded, while Windows and Linux artifacts did not.

The tagged 2.0 beta releases provide a validation path while physical Mac ↔ Windows Native Agent acceptance remains pending. A beta release does not establish physical acceptance or RC/stable readiness. A release pass must also run the full validation stack and packaged physical smoke appropriate to its actual claim.

## Repository hygiene

Preserve unrelated worktree changes. Review `git status --short`, `git diff --check`, and the exact staged diff. Do not commit generated fixtures, build output, credentials, or temporary audit notes. Use ProGraph for navigation, then source/compiler/tests for authority. Update the canonical document that owns a topic instead of adding another status narrative; keep release history in [CHANGELOG](../CHANGELOG.md).
