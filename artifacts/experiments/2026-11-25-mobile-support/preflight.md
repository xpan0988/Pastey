# Pre-flight record — open-ended mobile-support experiment

Observer: Claude Code (Opus 5.5, `claude-opus-5-5`), outside the collaboration.
Date: 2026-11-25. Status at the time of writing (≈14:50): not started, blocked on user decisions. Superseded: the run took place 15:07–16:09 and ended blocked; see `timeline.md`, `outcome.md`, `evaluation.md`.

Labels: **[repo]** read from the code at the baseline; **[runtime]** read from the installed native CLI or its config; **[observer]** my interpretation.

## Baseline

- Repository: `/Users/xiyuanpan/Pastey`, branch `main`, HEAD `3dec3dc` (2026-11-24 15:19:40 +1100). Working tree: only `.claude/launch.json` untracked. [repo]
- Pastey desktop app: not installed in `/Applications`; not running. Debug binary `src-tauri/target/debug/pastey` built 2026-11-24 15:19:00, 40 s before HEAD's commit time. [runtime]
- Unrelated processes present: stale `cargo test … codex_controller_` runs started 2026-09-15 (PIDs 61982, 61986, 62075, 62101, 62107). Not touched. [runtime]
- Claude Code CLI 2.1.318 (`~/.local/bin/claude`). Codex CLI 0.170.0 (`/opt/homebrew/bin/codex`). [runtime]

## Findings that affect the requested setup

1. **Automatic collaboration turns carry no model or effort.** `collaboration.rs::drive` starts every scheduled turn with `turn_options()`, which is `NativeSessionOptionsV1::default()` (model `None`, effort `None`) outside tests. Only a turn the user types in the conversation view can carry a chosen model/effort. [repo]
   - Codex: with no override, Codex uses `~/.codex/config.toml`: `model = "gpt-6.2-sol"`, `model_reasoning_effort = "high"` — matches the requested Codex configuration for every turn. [runtime]
   - Claude Code: Pastey's adapter notes that a chosen model persists for the session's later turns but "an effort is this turn's" (`claude_code/conversation.rs`, ~l.864). `~/.claude/settings.json` sets no `model` or `effortLevel`. So, without further configuration, automatic Claude turns run at Claude Code's default effort, not `xhigh`. [repo][runtime]
   - Claude Code 2.1.318 recognises an `effortLevel` setting and a `CLAUDE_CODE_EFFORT_LEVEL` variable (strings in the binary; the accepted values for the setting were not verified). A folder-local `.claude/settings.local.json` in the Claude worktree could pin model and effort without changing Pastey. [runtime][observer]
2. **Claude Code member turns are headless `claude -p` with no permission mode.** Anything that would prompt (file edits, most shell commands) is declined and the turn goes on (`conversation.rs` header; M1/M2 live runs: "write to OrdersTable.js was declined"). Pastey never answers native approvals. Under the current native permissions the Claude participant can read and reason but cannot edit files or run most commands. [repo]
3. **The tracked project settings allow, without a prompt:** `Bash(git push origin claude/*)` and `Bash(gh pr create *)` (`.claude/settings.json` at HEAD). A Claude participant in a worktree of this repo inherits them, so it could push a `claude/*` branch or open a PR without any approval. This conflicts with "no push without explicit user authorization". [repo][observer]
4. **Codex member turns** start `thread/start` with only `cwd`; sandbox and approval policy are Codex's defaults for that folder. A new worktree path is not in `~/.codex/config.toml`'s trusted projects. Any Codex approval request pauses that turn and appears under Pastey's Needs you; Pastey never answers it. Under the protocol I may not answer it either — each one goes to the user. [repo][runtime]
5. **Interface to drive.** The only real interface is the Pastey desktop app (`npm run tauri:dev`, the user's real Pastey app data), driven through its UI. `tools/ui-bridge` is a test harness (mock runtime, two in-process Hosts) that forces collaboration turns to effort `low`, so it is not admissible here. [repo][observer]
6. **Grant bounds** match the request: max 20 turns / 24 h per member; at most 30 automatic turns per hour per device. [repo]
7. **Quota.** Both participants use this Mac's own accounts. The Claude participant shares its usage limit with the user's other Claude sessions, including this observer session. 40 long agentic turns on this repository at `xhigh`/`high` is a large draw. [observer]

## Conflicts with stored preferences (noted, not acted on)

- Memory "lowest effort for other Agents" — overridden by this experiment's explicit `xhigh`/`high` requirement.
- Memory "main-only repo, never create branches" — overridden by this experiment's explicit worktree/branch requirement; nothing will be merged into `main`.
