# P02: repeat of P01 after the CF-1 fix (2026-11-26)

Observer: Claude Code, Opus 5.5, outside the collaboration. Pastey at `48e4fb9` (P01 code + the fix that keeps a message quoting fenced code whole). The user authorised this run before leaving and asked not to be consulted. Full evidence: `~/research-playgrounds/p02/evidence/`. This folder holds the shareable subset.

Labels: **[pastey]** Journal/IPC; **[runtime]** native transcript or rollout; **[observer]** my action or reading; **[verified]** I reran it against the frozen verifier.

## Result in one paragraph

Same objective, models, effort, profile, budget, kickoff text and baseline as P01, with fresh clones. The collaboration ran from 17:26:29 to 17:32:01 and then stopped on its own: 5 min 32 s of activity. Nobody intervened after release. Plan r1 was adopted after 35 s. The best result is **96 Toffoli / 67 qubits**, against the 953 / 71 incumbent. This time the two members cross-reproduced through Pastey alone. Claude sent its 24-term GF(256) multiplier inline in a Pastey message. Codex checked it on all 65,536 input pairs and regenerated the 96/67 circuit byte for byte. I regenerated all six claimed stage circuits independently, and each passes the unmodified verifier. Taken across P01 and P02, the research finding is robust: two independent runs each cut the L=3 cost by about 90 % in a few minutes, by different routes. P01 reached 99 / 65 and P02 reached 96 / 67.

## Deviation: how P02 was launched

The user's screen locked at 17:07:50 (`CGSSessionScreenIsLocked = 1`) and the request for full-screen control went unanswered, so the Pastey desktop window could not be driven. P02 ran instead through Pastey's own development IPC bridge (`src-tauri/src/collaboration/ui_bridge.rs`, `cargo test --bin pastey collaboration_screens_over_a_browser_bridge -- --ignored`). The bridge serves the app's real Tauri commands on 127.0.0.1:17431, through Tauri's IPC dispatch and with a real Host. That Host runs the real collaboration service, readiness probes and scheduler, and starts the installed Codex 0.170.0 and Claude Code 2.1.332.

What differs from P01:
- **App data:** a test Host with temporary app data, not the user's.
- **Model and effort:** the harness forces every collaboration turn to effort `low` with no model (`TURN_OPTIONS_FOR_TESTS`). Claude's model was set through `ANTHROPIC_MODEL=claude-sonnet-5-5` in the bridge's clean environment, and the transcript confirms `claude-sonnet-5-5` on every message. Codex used `~/.codex/config.toml`, and the rollout shows `gpt-6.2-sol`, effort `low`, on every turn. The effort values were the same as P01's, but they came from the harness rather than from member options.
- **Commands instead of screens:** I called the same commands the screens call (`create_collaboration`, `set_collaboration_execution`, `set_collaboration_grant`, `check_collaboration_readiness`, `send_collaboration_message`, `release_collaboration`, `set_collaboration_state`). Helper: `raw/ipc.sh`.
- **Time limit:** the bridge stops itself after one hour (18:25). The run needed 11 minutes.

## Configuration

| Item | Value |
|---|---|
| Workspaces | Fresh clones `p02/ws/{claude,codex}` of `qcc-stage-playground`, branch `p02/<alias>` at `572572f`, origin removed, `OPERATOR.md` removed (`7b676c9` / `2d2056c`) |
| Execution | Unattended engineering, collaboration scope; Codex read-only while planning, then workspace-write (rollout `turn_context`); Codex's auto-review allowed 3 local `git commit`s ("no push or external side effect") |
| Readiness | Both members ready before release (`raw/readiness.json`) |
| Grants | 7 automatic turns / 2 h each; plan-first on |
| Kickoff | P01's text verbatim, with "P02" |

## Timeline

| Time | Basis | Event |
|---|---|---|
| 17:26:29 | pastey | Released |
| 17:26:48 | pastey | claude's own plan block refused ("task A is over its bounds, or claims a path outside its folder"); claude never re-proposed |
| 17:27:00 | pastey | codex proposes r1 (tasks: schedule for codex, gadgets for claude, integration for codex after schedule) |
| 17:27:04 | pastey | claude accepts → **r1 adopted** (35 s) |
| 17:28:19 | pastey/verified | claude: 24-AND GF(256) multiplier (exhaustive component PASS), stage **896 / 71** |
| 17:29:11 | pastey/verified | codex: closed-form schedule **115 / 71** and **123 / 67** (`ce16424`); Pastey tells codex its dependent task is ready |
| 17:30:07 | pastey/verified | claude: + 36-T inversion, stage **816 / 69**; proposes both gadgets for codex's schedule |
| 17:30:14 | pastey/verified | codex: compact-norm **99 / 67**; asks claude to send the multiplier source through Pastey |
| 17:30:24 | pastey | claude sends the 24 terms inline (1,224 B), delivered whole |
| 17:31:19 | pastey/verified | codex: **96 / 67** (`2c8959a`) with a 24-term multiplier it derived independently (interpolation over GF(4)), before claude's message reached it |
| 17:31:58 | pastey/runtime | codex: claude's exact terms regenerate the same 96/67 circuit byte for byte (ledger note "Claude multiplier terms received via Pastey") |
| 17:32:01 | pastey | last turn; quiet afterwards |
| 17:37:20 | observer | after 5 quiet minutes, ended via `set_collaboration_state` |

## Research outcome [verified]

| L=3 point | T | Q | Author | Command | SHA-256 prefix |
|---|---:|---:|---|---|---|
| incumbent | 953 | 71 | — | — | — |
| fov + mul24 | 896 | 71 | claude | `work/claude/gen_stage24.py` | d2ec56c510f3 |
| fov + mul24 + inv36 | 816 | 69 | claude | `work/claude/gen_stage24_inv36.py` | 871d8e246a50 |
| closed form, split inverse | 115 | 71 | codex | `closed_form.py --split-inverse` | 5a7907733c55 |
| … + recycle t6 | 123 | 67 | codex | `closed_form.py --split-inverse --recycle-t6` | 2cec1f4f5848 |
| … + compact norm | 99 | 67 | codex | `closed_form.py --split-inverse --compact-norm` | b91a6cfe088a |
| … + 24-term multiplier | **96** | **67** | codex (claude's terms reproduce it) | `closed_form.py --split-inverse --compact-norm --mul24` | 860c072519ff |

Pareto front across P01 and P02: **(96 T, 67 Q)** from P02, **(99 T, 65 Q)** from P01 and **(198 T, 61 Q)** from P01. No construction has yet combined P02's 24-term multiplier with P01's 65-qubit layout. That combination is an obvious next experiment, and it may be cheap.

Caveats: the same as P01. An expert should confirm that the valid-domain specification is the intended problem, and that a ~90 % reduction found in minutes is not a property of how the problem is framed. Everything is L=3-specific, and no L=4 validation has been run.

## Collaboration evaluation

Compared with P01:

| Measure | P01 | P02 |
|---|---|---|
| Release → adopted plan | 42 s | 35 s |
| Active duration (release → last turn) | 4 min 35 s | 5 min 32 s |
| Turns claude (planning / execution) | 2 / 5 | 2 / 5 |
| Turns codex (planning / execution) | 2 / 4 | 1 / 4 |
| Longest turn | ~75 s | ~75 s |
| Messages lost | 1 (CF-1) | 0 |
| Cross-reproduction through Pastey only | none (claude read codex's folder; codex got nothing) | yes: codex reproduced 96/67 from claude's message |
| Pastey problems raised | 1 | 3 (1 refused plan block, 2 false claim conflicts) |
| Tokens claude (out / cache-read) | 16.8k / 1.47M | 19.1k / 1.34M |
| Tokens codex (out / input) | 9.7k / 1.16M | 10.9k / 1.47M |
| Human interventions after release | 0 | 0 |
| Best verified | 99 / 65 | 96 / 67 |

What improved: messages arrived complete, and the members completed one exchange of a gadget through Pastey followed by a reproduction. Codex asked for the source, claude sent it, and codex verified the gadget and reproduced the circuit. The four-backtick path of the fix was not exercised live, because claude sent its data without code fences. The fix stands on its unit tests.

New or remaining Pastey frictions:
- **False claim conflicts (2).** R5 compares claimed paths literally across members. Each member writes `results/ledger.jsonl` in its own clone, yet Pastey reported that claude changed a file claimed by codex. Claims need to be keyed by folder, or by repository identity.
- **A refused plan block is not explained to its author.** Claude's first proposal was rejected with a message that names two possible causes. Claude never repaired it and simply accepted codex's plan. Nothing in the evidence shows that claude saw the reason.
- **`done` artifacts for uncommitted work.** Claude reported `p02/claude@work-tree-uncommitted` three times, and Pastey answered "not a commit id; not checked". Claude committed nothing, in either run.
- **Filesystem handoff is still suggested.** Claude again told codex to "copy work/claude/*.py into your folder". Codex kept to Pastey messages.
- **Early quiescence is still not signalled** to the user. The adoption notice still spends a grant turn (6 grant turns were used by claude: 1 planning + 5 execution).

Guard rails: protected files are unchanged in both clones and the frozen playground is clean. No L=4 attempt was made (no `REFUSED` output), nothing was pushed, and no subagents were used.

## Files

- `raw/journal-export.jsonl` (52 entries), `raw/collaboration-view-final.json`, `raw/readiness.json`, `raw/presets.json`, `raw/ipc.sh`, `raw/journal_poll.py`
- `observer-validation.md`, `observations.md`, `setup-log.md`, `hashes-*.txt`
- `raw/worktrees/{claude,codex}-work.tgz`, `*-commits.txt`, `*-status.txt`
- Kept outside the repository, in `~/research-playgrounds/p02/evidence/raw/`: the native transcript and rollouts (`runtime/`) and the bridge log
