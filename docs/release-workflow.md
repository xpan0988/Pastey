# Release Workflow

Pastey uses `src-tauri/Cargo.toml` as the authoritative app version.

## Bump Version

Run:

```bash
npm run release:version -- 1.5.1
```

This updates the Rust crate version, syncs required app/package version files, updates `CHANGELOG.md`, runs version checks, creates a commit, and creates an annotated Git tag.

## Add Release Title

Run:

```bash
npm run release:version -- 1.5.1 "Transfer Pipeline and Speed Policy"
```

The changelog heading becomes:

```md
## 1.5.1 — Transfer Pipeline and Speed Policy — YYYY-MM-DD
```

## Dry Run

Preview planned edits without changing files:

```bash
npm run release:version -- 1.5.1 --dry-run
```

## Dirty Working Tree

The release command refuses to run with uncommitted changes. To intentionally run anyway:

```bash
npm run release:version -- 1.5.1 --allow-dirty
```

The script stages only the release files it edits.

## Push Release

The script does not push automatically. After reviewing the commit and tag, run:

```bash
git push origin main --tags
```

## GitHub Actions

The release build is triggered by the pushed tag, for example `v1.5.1`. The version check compares the tag version against the internal app version before the release build continues.
