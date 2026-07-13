# Development and release

This document owns development, validation, release, and documentation-maintenance procedures. Architecture belongs in [architecture.md](architecture.md) and the corresponding layer documents.

## Setup and development

```bash
npm install
npm run tauri:dev
```

Use `npm run tauri:dev-fast` only for local transfer-throughput testing. Build the frontend with `npm run build`; build a desktop package with `npm run tauri:build`; use `npm run build:checked` for a packaged build plus version and artifact checks. Linux release hosts can use `npm run build:checked:linux`.

## Ordinary checks

```bash
git diff --check
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
node scripts/run-transform-tests.mjs
```

Focused TypeScript and integration runners live under `scripts/run-*-tests.mjs`. Useful examples include:

```bash
node scripts/run-ai-slot-tests.mjs
node scripts/run-file-candidate-tests.mjs
node scripts/run-candidate-payload-tests.mjs
node scripts/run-candidate-payload-workflow-tests.mjs
node scripts/run-capability-manifest-tests.mjs
node scripts/run-room-agent-bridge-tests.mjs
```

## Transfer and Layer 4 validation

Replay deterministic planner scenarios with:

```bash
node scripts/replay-transfer-planner-scenarios.mjs
node scripts/run-transfer-planner-tests.mjs
node scripts/run-cl4-contention-smoke.mjs
node scripts/run-layer4-validation-matrix.mjs
```

The contention smoke exercises the production control-demand reducer, planner allocation, active binary-v1 window update, and room-control suites. The Layer 4 matrix covers ordinary-data routing, queue children/terminal state, selected-peer control/capability policy, consent boundaries, and durable-display/reconnect boundaries. Neither is a substitute for a GUI or two-device LAN run.

## Transform and provider validation

Run `node scripts/run-transform-tests.mjs` for the frontend Transform contract. Rust tests cover the unavailable production adapter, Transform consent/journal/sanitizer boundaries, descriptor staging, capability probe, and behavioral-verifier model.

Provider health checks are Settings-driven and advisory-only: use a deliberately minimal configured request, verify validation reports, and confirm it neither creates a Bridge control event nor changes consent/execution state. Do not put API keys in fixtures, logs, or documentation.

Unit/mock behavioral verification **is not** live Linux isolation verification. The Stage 2B verifier foundation and feature-gated test probe are test infrastructure. A future live Linux check must demonstrate the configured static prerequisites and behavioral filesystem, network, seccomp, cgroup, process-tree, bounded-output, and cleanup controls. Until that evidence exists, production Transform must remain `sandbox_unavailable`.

## Smoke checks

For a single-machine dual-instance smoke, launch two isolated local app instances, create/join a Bridge, verify selected-peer ordinary data, selected-peer control delivery, a denied request, a Search request, and the selected-file Return second-consent boundary. Use generated transfer fixtures only when testing scheduler behavior.

For a two-device smoke, repeat on two supported desktop devices over the same LAN, exercise nearby/code join, an ordinary file transfer, disconnect/reconnect route expiry, and a Burn. Verify that paired-device display identity neither auto-joins nor authorizes a capability. Packaged release smoke remains required for release confidence.

The transfer fixture corpus is documented in [tests/fixtures/transfer-corpus/README.md](../tests/fixtures/transfer-corpus/README.md). Generated payloads are local-only and must not be committed.

## Release procedure

`src-tauri/Cargo.toml` is the authoritative app version. Release with:

```bash
npm run release:version -- X.Y.Z "Release Title"
git push origin main --tags
```

The script requires a greater unused version, updates derived version files and release documentation, runs its built-in checks, stages only its release-file edits, creates `chore(release): vX.Y.Z`, and creates annotated tag `vX.Y.Z`. Use `--dry-run` to preview. It refuses a dirty worktree unless `--allow-dirty` is explicit; review the final diff in that case. The script does not push automatically.

Its built-in checks are Cargo formatting, Cargo check, and `npm run check:version`. For a full release pass also run Cargo tests, `npm run build`, and `npm run build:checked`. On tag builds, version checks compare the tag against the Cargo version before release builds continue.

## Git and documentation hygiene

Keep unrelated worktree changes intact. Review `git status --short`, `git diff --check`, and the exact staged diff before committing. Do not commit generated fixtures, build output, or secrets.

For repository navigation, use ProGraph first when cross-file relationships matter, then verify behavior in source and tests. Documentation follows the ownership map in [architecture.md](architecture.md): one current architectural topic has one canonical owner. Update the owner rather than adding duplicate current-status narratives; keep historical release sequence in [CHANGELOG.md](../CHANGELOG.md). Verify Markdown links and search for removed paths before finishing a documentation change.
