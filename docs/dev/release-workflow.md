# Release Workflow

Pastey uses `src-tauri/Cargo.toml` as the authoritative app version. All other app/package version files should be derived from it during a release bump.

Detailed product update history lives in [version-history.md](version-history.md).

## Bump Version

Run:

```bash
npm run release:version -- X.Y.Z
```

The version must be greater than the current `src-tauri/Cargo.toml` version, and the matching Git tag must not already exist.

The script updates:

- `src-tauri/Cargo.toml`
- `package.json`
- `package-lock.json`, including the root package entry
- `src-tauri/tauri.conf.json`, including `package.version` if present
- `src-tauri/Cargo.lock` for the `pastey` package
- `CHANGELOG.md`
- `docs/release-notes/vX.Y.Z.md` only when `docs/release-notes/` exists

It then runs its built-in checks, stages only the release files it edits, creates a commit, and creates an annotated Git tag.

## Add Release Title

Run:

```bash
npm run release:version -- X.Y.Z "Release Title"
```

The changelog heading becomes:

```md
## X.Y.Z — Release Title — YYYY-MM-DD
```

Without a title, the heading is:

```md
## X.Y.Z — YYYY-MM-DD
```

## Dry Run

Preview planned edits without changing files:

```bash
npm run release:version -- X.Y.Z --dry-run
```

Dry runs print the current version, next version, planned file edits, built-in checks, commit message, and tag name. They do not modify files.

## Dirty Working Tree

The release command refuses to run with uncommitted changes. To intentionally run anyway:

```bash
npm run release:version -- X.Y.Z --allow-dirty
```

With `--allow-dirty`, unrelated working tree changes remain unstaged unless they are in a release file the script edits. Review the final diff before pushing.

## Verification

The release script currently runs:

```bash
(cd src-tauri && cargo fmt --check)
(cd src-tauri && cargo check)
npm run check:version
```

For a full release pass, also run the broader validation stack when feasible:

```bash
(cd src-tauri && cargo test)
npm run build
npm run build:checked
```

`npm run check:version` verifies that `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.lock` match the authoritative `src-tauri/Cargo.toml` version. On tag builds, it also checks the Git tag version.

## Commit and Tag

The release commit uses:

```text
chore(release): vX.Y.Z
```

The annotated tag uses:

```text
vX.Y.Z
```

## Push Release

The script does not push automatically. After reviewing the commit and tag, run:

```bash
git push origin main --tags
```

## GitHub Actions

The release build is triggered by the pushed tag, for example `vX.Y.Z`. The version check compares the tag version against the internal app version before the release build continues.
