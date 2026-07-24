# Development and release

This document owns development, validation, release, and documentation-maintenance procedures. Architecture belongs in [architecture.md](architecture.md) and the corresponding layer documents.

## Setup and development

```bash
npm install
npm run tauri:dev
```

Use `npm run tauri:dev-fast` only for local transfer-throughput testing. Build the frontend with `npm run build`; build a desktop package with `npm run tauri:build`; use `npm run build:checked` for a packaged build plus version and artifact checks. Linux release hosts use `npm run build:checked:linux` for Linux installers.

## Ordinary checks

```bash
git diff --check
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
node scripts/run-layer4-validation-matrix.mjs
```

Focused TypeScript and integration runners live under `scripts/run-*-tests.mjs`. Useful examples include:

```bash
node scripts/run-natural-v1-tests.mjs
```

## Transfer and Layer 4 validation

Replay deterministic planner scenarios with:

```bash
node scripts/replay-transfer-planner-scenarios.mjs
node scripts/run-transfer-planner-tests.mjs
node scripts/run-layer4-validation-matrix.mjs
```

The Layer 4 matrix covers ordinary-data routing, active Bridge-detail polling, queue children/terminal state, and durable-display/reconnect boundaries. It is not a substitute for a GUI or two-device LAN run.

## Transform and provider validation

For the live product, run `cargo test --manifest-path src-tauri/Cargo.toml bridge_plan:: -- --nocapture`; it covers durable Plan lifecycle, receiver protocol review, Search grants, and bounded Transform/Transfer bindings. The live readable-text Transform uses Rust-private immutable staging and a fixed worker; no legacy Transform consent, journal, adapter, or command path remains.

Provider health checks are Settings-driven and advisory-only: use a deliberately minimal configured request, verify validation reports, and confirm it neither creates a Bridge control event nor changes consent/execution state. Do not put API keys in fixtures, logs, or documentation.

Linux capability probes and behavioral verification remain dormant test infrastructure for a future verified execution backend. The optional verification probe binary is feature-gated and is not packaged with the app. Neither has product authority, UI state, command surface, sidecars, or an availability signal. Unit, cross-compile, mock, and packaging verification **are not** live Linux isolation verification; a future backend requires its own explicit product and security decision before it can be installed.

## Smoke checks

For a single-machine dual-instance smoke, launch two isolated local app instances, create/join a Bridge, verify selected-peer ordinary data, selected-peer control delivery, a denied Bridge Plan, a Search plan, then an approved selected-file Transfer. Exercise both Transfer to the requester and the selected-device Pastey Shared location when possible. Use generated transfer fixtures only when testing scheduler behavior.

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
