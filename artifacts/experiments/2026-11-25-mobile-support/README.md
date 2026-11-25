# Evidence package — open-ended mobile-support experiment (2026-11-25)

Result: **blocked**. Two native Agents (Claude Code / Sonnet 5.5, Codex / GPT-6.2-sol) in one Pastey collaboration worked one turn each without ever exchanging a delivered message. codex stalled on a native approval that Pastey did not surface.

- `experiment-config.md`: baseline, identities, models, permissions, collaboration, controls, interventions
- `preflight.md`: setup blockers found before the run
- `timeline.md`: chronological record with basis labels
- `metrics.json`: quantities, with nulls for what was unavailable
- `outcome.md`: what each Agent understood, designed and changed
- `evaluation.md`: assessment of Pastey as a governance layer, recommendations, next considerations
- `raw/`: Journal export (`journal-export.jsonl`), poller output, worktree patches and untracked-file archives, codex's escalated script, the observer's poller script

Cleanup done: worktrees and `exp/*` branches removed, the trust record and exclude line reverted, build bundle and temp files removed, collaboration deleted in Pastey. `main` was never touched (`3dec3dc`).

Not committed (kept on the experiment Mac only, see `.gitignore`): the Claude Code session transcript `raw/runtime/e79e950b-….jsonl` and the Codex rollout `raw/runtime/rollout-2026-11-25T15-08-37-….jsonl`. Both hold the user's email, provider account ids and full session context. The Pastey launch and build logs (`raw/*.log`) are also local only, under the repository's `*.log` rule. Figures taken from them are in `metrics.json` and `timeline.md`.
