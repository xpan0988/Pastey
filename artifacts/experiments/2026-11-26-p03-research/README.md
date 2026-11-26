# P03: verifying 5a23537 under P02's conditions (2026-11-26)

Observer: Claude Code, Opus 5.5, outside the collaboration. Pastey at `5a23537`. That commit compares claims within each member's folder and tells a member which of its blocks were not taken. It also carries `48e4fb9`, the CF-1 fix that lets a message quote fenced code. The user asked for a background run. Full evidence: `~/research-playgrounds/p03/evidence/`.

## Setup

P03 used the same launch path as P02: Pastey's IPC bridge (`collaboration/ui_bridge.rs`). Behind it are a real Host, the real collaboration service, real readiness probes and scheduler, and the installed Codex and Claude Code. Its limits are the same as in P02: a test Host's temporary app data, every turn forced to effort `low`, Claude's model set through `ANTHROPIC_MODEL=claude-sonnet-5-5`, and a one-hour cap. Everything else matched P01 and P02: fresh clones at `572572f` with `OPERATOR.md` removed, the Unattended engineering profile at collaboration scope, grants of 7 turns / 2 h, plan-first, and the same kickoff text. The rollout and transcript confirm `gpt-6.2-sol`/low (read-only while planning, workspace-write afterwards) and `claude-sonnet-5-5` on every message.

## What happened

Released at 17:56:59; last turn at 18:03:09 (6 min 10 s of activity); ended by the observer at 18:08 after 5 quiet minutes. Nobody intervened after release.

| Time | Event |
|---|---|
| 17:57:14 | claude proposes r1: four tasks, paths `work/claude/**` and `work/codex/**` |
| 17:57:42 | codex proposes r2: two tasks, `work/codex/` and `work/claude/` |
| 17:57:45 | claude accepts → **r2 adopted** (46 s) |
| 17:57:54 | codex accepts r2 and asks claude to accept it too. The request is stale: r2 was already adopted (the same race as P01) |
| 17:59:08 | codex: direct stage **230 / 63** |
| 17:59:19 | claude: **203 / 63**, by paying the bilinear phase with conditioned CZ gates |
| 17:59:49 | claude: **167 / 63**, by erasing the inverses with a phase-only inversion |
| 18:00:52 | codex: **159 / 63** and **115 / 75**; also re-derives claude's 203/63 method independently (different hash) |
| 18:01:10 | claude sends the **exact 203/63 generator as a fenced Python block** inside its `pastey-message` (2,879 B). It arrived whole |
| 18:01:47 | codex: **123 / 71**, **141 / 63** |
| 18:03:01 | codex regenerates claude's 203/63 **with a matching SHA-256** from the delivered generator; split-output variants **115 / 71**, **123 / 67**, **141 / 59** |
| 18:03:09 | last turn |

## Do the fixes hold?

| Fix | Exercised live? | Evidence |
|---|---|---|
| CF-1: a message may quote fenced code (`48e4fb9`) | **Yes** | Journal #38 contains a ```` ```python ```` block and was delivered in full. Codex rebuilt `f24db08c…b686` from it byte for byte (#46). In P01 the same kind of message was dropped |
| Claims compared within members' folders (`5a23537`) | No | Neither plan claimed a path both members also wrote (claims were `work/<alias>/`), so nothing could have been flagged either way. No false conflict was raised (0 attentions) |
| Refused blocks told to their author (`5a23537`) | No | No block was refused in P03 (0 attentions). The behaviour rests on `collaboration::tests::claims_in_separate_folders_never_cross_and_a_refused_plan_block_reaches_its_author` |

Not caused by the fixes, and unchanged: a stale acceptance request after adoption; the "plan adopted" notice spends a grant turn; claude committed nothing; quiescence is not signalled to the user.

## Research outcome [verified]

I regenerated ten stage circuits from the participants' generators in a fresh clone of the frozen playground. Every SHA-256 matches its ledger entry and every circuit passes the unmodified verifier (`observer-validation.md`). Selected points:

| L=3 point | T | Q | Author | Command |
|---|---:|---:|---|---|
| split output, phase-first | 141 | **59** | codex | `retained.py out --retain split-phase` |
| split output | **115** | 71 | codex | `retained.py out --retain split` |
| split output, partial | 123 | 67 | codex | `retained.py out --retain split-partial` |
| phase-only inverse erase | 167 | 63 | claude | `gen_phase_direct2.py` |
| bilinear phase via conditioned CZ | 203 | 63 | claude (reproduced by codex) | `gen_phase_direct.py` |

The **Pareto front across P01–P03** is now (96 T, 67 Q) from P02, (99 T, 65 Q) from P01 and **(141 T, 59 Q)** from P03. P03's point replaces P01's (198 T, 61 Q). P03 is the third independent run to find a construction far below the 953 / 71 incumbent. Each run took a different route, which strengthens the case for a real structural saving. The caveats in the P01 and P02 reports still apply: an expert should check that the valid-domain specification is the intended problem, and L=4 generality has not been checked.

## Measures

| Measure | P01 | P02 | P03 |
|---|---|---|---|
| Release → adopted plan | 42 s | 35 s | 46 s |
| Active duration | 4:35 | 5:32 | 6:10 |
| Turns claude (planning / execution) | 2 / 5 | 2 / 5 | 2 / 5 |
| Turns codex (planning / execution) | 2 / 4 | 1 / 4 | 2 / 4 |
| Messages lost | 1 | 0 | 0 |
| Pastey attentions | 1 | 3 (2 false conflicts) | 0 |
| Cross-reproduction through Pastey | none | codex ← claude (inline terms) | codex ← claude (fenced generator) |
| Tokens claude (out / cache-read) | 16.8k / 1.47M | 19.1k / 1.34M | 18.8k / 1.23M |
| Tokens codex (out / input) | 9.7k / 1.16M | 10.9k / 1.47M | 10.2k / 1.13M |
| Stage results verified by observer | 5 | 6 | 10 |
| Commits by members | codex 3 | codex 3 | none |

## Files

- `raw/journal-export.jsonl` (52 entries), `raw/collaboration-view-final.json`, `raw/readiness.json`, `raw/ipc.sh`, `raw/journal_poll.py`
- `observer-validation.md`, `setup-log.md`, `hashes-*.txt`, `raw/worktrees/*`
- Kept outside the repository: the native transcript and rollout, the bridge log and resource samples (`~/research-playgrounds/p03/evidence/raw/`)
