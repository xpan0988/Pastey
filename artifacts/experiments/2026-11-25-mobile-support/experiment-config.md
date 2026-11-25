# Experiment configuration

Open-ended multi-Agent engineering experiment through Pastey. Started 2026-11-25. Observer: Claude Code, Opus 5.5 (`claude-opus-5-5`), outside the collaboration.

Basis labels: **[pastey]** observed in Pastey (Journal / UI / its DB); **[runtime]** reported by the native runtime; **[observer]** my action or interpretation; **[user]** the human user's decision.

## Baseline and environment

| Item | Value |
| --- | --- |
| Repository | `/Users/xiyuanpan/Pastey` (main checkout, not a participant workspace) |
| Baseline commit | `3dec3dced906aa1d0ea3afc5b41dfd7d1766635c` (`main`, 2026-11-24 15:19:40 +1100) |
| Host | One physical Mac, macOS 26 (Darwin 25.5.0), Apple silicon |
| Pastey | Debug `.app` bundle built from the baseline (`npm run tauri -- build --debug --bundles app`), product version 2.0.0, identifier `dev.localfirst.pastey`, the user's real Pastey app data. Launched detached with a clean environment (`env -i` + HOME, USER, LOGNAME, SHELL, LANG, TMPDIR, PATH incl. `~/.local/bin`, `/opt/homebrew/bin`, `~/.cargo/bin`). |
| Claude Code | 2.1.318, `~/.local/bin/claude`, user's own sign-in |
| Codex | 0.170.0, `/opt/homebrew/bin/codex`, user's own sign-in |
| Driving interface | Pastey's own UI via macOS computer use (background control; full-screen control approved by the user for the folder picker and grant dropdowns) |

## Participants

| | claude | codex |
| --- | --- | --- |
| Pastey alias | `claude` | `codex` |
| Provider | Claude Code | Codex |
| Folder (git worktree) | `/Users/xiyuanpan/Pastey-exp-claude` | `/Users/xiyuanpan/Pastey-exp-codex` |
| Branch | `exp/mobile-claude` @ `3dec3dc` | `exp/mobile-codex` @ `3dec3dc` |
| Role line | empty | empty |
| Requested model / effort | Claude Sonnet 5.5, `xhigh` | GPT-6.2-sol, `high` |
| Model as observed | `claude-sonnet-5-5` (probe init + session transcript) [runtime] | `gpt-6.2-sol` (rollout `turn_context`) [runtime] |
| Effort as observed | `xhigh` configured via `effortLevel`; **not observable** in any Claude Code output [runtime] | `high` (rollout `turn_context`) [runtime] |
| How model/effort are held | Folder-local `.claude/settings.local.json` (I-1), because Pastey's automatic turns pass none | `~/.codex/config.toml` defaults (`model`, `model_reasoning_effort`), unchanged |
| Native permissions | `-p` headless; `permissionMode=acceptEdits` (I-1); deny `git push`, `gh pr`, `gh release` (I-1); tracked project allow-rules remain (`git push origin claude/*` is now overridden by the deny) | `approval_policy=on-request`; sandbox `workspace-write`, `network_access=false` (Codex defaults for this folder) |
| Session identity | Claude session `e79e950b-229e-4af2-a167-f2859e9f766f` (Pastey `--session-id`) | Codex thread rollout `rollout-2026-11-25T15-08-37-01a210e4-…` (originator `Pastey`) |
| Peer-message isolation | `--settings {"crossSessionInbound":"refuse"}` on every member turn [runtime] | n/a |

## Collaboration

| Item | Value |
| --- | --- |
| Id | `collaboration:d90fde84-ae21-473b-b861-8d65a7880da1` |
| Name | `mobile-support` |
| Goal (one line all members see) | "Investigate, design, and implement mobile support for Pastey." |
| Objective delivered | The same sentence, as a user message to each member: codex 15:08:35, claude 15:14:09 (first attempt 15:09:09 failed, see timeline) |
| Grants | 20 automatic turns within 24 h each (codex until 2026-11-26 15:08:01; claude until 15:08:21) |
| Pastey limits in force | ≤ 30 automatic turns/hour/device; hop limit 4; ≤ 3 message blocks + 1 summary per turn; message ≤ 4 KiB; summary ≤ 2 KiB; ≤ 20 waiting messages per member; a user message does not consume a grant |

## Experimental controls

- No requirements, platform, technology, architecture, roles or scope given to either Agent beyond the objective. Role lines left empty; aliases chosen to name only the provider.
- No manual relaying; all inter-Agent communication only through Pastey's message blocks.
- The observer does not answer native approvals or Agent questions; each is put to the user.
- Nothing merged into `main`, pushed or released.
- Participants' implementation not edited by the observer.

## Interventions before the run (all user-authorised)

| Id | Time | What | Why | Effect on validity |
| --- | --- | --- | --- | --- |
| I-1 | 14:55 | Claude worktree `.claude/settings.local.json`: model, `effortLevel: xhigh`, `acceptEdits`, deny push/PR/release | Pastey automatic turns carry no model/effort; headless Claude declines edits; tracked settings allow `git push origin claude/*` | Claude's permissions differ from a default install and from Codex's |
| I-2 | 14:58 | `.claude/settings.local.json` added to shared `.git/info/exclude` | Keep observer config out of participant commits | None on behaviour |
| I-3 | 15:13 | Trust record for `~/Pastey-exp-claude` written by hand into `~/.claude.json` | Pastey refused to start Claude Code there; Claude Code offered no trust prompt for the worktree | claude started ~5.5 min after codex |

## Deviations from the requested setup

- Claude effort `xhigh` cannot be verified from runtime output.
- Asymmetric native permissions (see I-1).
- claude started 5 min 34 s after codex.
- Pastey runs as a debug build of the baseline, not a release package.
- Memory preferences overridden for this experiment by the explicit request: "lowest effort for other Agents" and "main-only, never branch".
