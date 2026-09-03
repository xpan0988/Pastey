# Upstream provenance

This crate is derived from `openai/codex` at commit
`ddf8a67ab09cd76b8adc0969f11ee1271179aba7`.

Retained source trees:

- `codex-rs/windows-sandbox-rs/` -> this crate;
- `codex-rs/utils/pty/` -> `../codex-utils-pty/`;
- `codex-rs/utils/absolute-path/` -> `../codex-utils-absolute-path/`;
- `codex-rs/utils/string/` -> `../codex-utils-string/`.

The upstream Apache-2.0 `LICENSE` and `NOTICE` are included in this directory.
Pastey's adapter is outside this crate in
`src-tauri/src/windows_codex_backend.rs`. Pastey authority types do not enter
the derived crate.

## Update procedure

1. Check out the intended `openai/codex` commit and record its full hash here.
2. Diff the four retained upstream source trees against their counterparts in
   this repository. Review security-sensitive upstream changes before copying.
3. Copy upstream files while preserving paths and module structure. Reapply
   only the divergences recorded in `PATCHES.md`; do not resolve conflicts by
   introducing Pastey policy into this crate.
4. Refresh the standalone manifests and the minimal private product-type
   extraction only as required by current upstream signatures.
5. Run formatting, all applicable derived-crate tests, the Windows GNU
   compile-only check, Pastey's focused ExecutionWorld tests, and Pastey's full
   Rust/frontend validation.
6. On a configured native Windows Host, run the complete Stage 1–5 acceptance
   procedure in `docs/development.md` before treating the new upstream revision
   as available in production.

Cross-compilation and upstream unit tests do not constitute native Windows
confinement evidence.
