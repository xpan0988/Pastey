# P01: research collaboration through Pastey (2026-11-26)

Observer: Claude Code, Opus 5.5 (`claude-opus-5-5`), outside the collaboration. Pastey at `a2ce090` (debug bundle, real app data). Full evidence: `~/research-playgrounds/p01/evidence/` (this folder holds the shareable subset; native transcripts stay there because they contain account identifiers).

Labels: **[pastey]** Journal/UI/DB; **[runtime]** native transcript or rollout; **[observer]** my action or reading; **[verified]** I reran it against the frozen verifier.

## Result in one paragraph

The collaboration ran autonomously, with no human intervention after release, from 16:26:11 to 16:30:46 and then stopped on its own: the plan task was done and no messages were pending. That is 4 min 35 s of a 60-minute allowance, and every turn took between 4 and 75 seconds. Plan r2 was adopted 42 s after release. The two members produced five full-tier-verified L=3 stage circuits. The best is **99 Toffoli / 65 qubits**, against the 953 / 71 incumbent: −89.6 % T and −6 Q. I regenerated all five byte-identically from the participants' generators in a fresh clone of the frozen playground, and each passes the unmodified verifier. Collaboration effectiveness was mixed. Exchange through Pastey worked fast. One message was silently lost to a block-format defect, and as a result cross-reproduction ran only one way: claude reproduced all of codex's results, but codex reproduced none of claude's.

## Configuration

| Item | Value |
|---|---|
| Workspaces | Independent clones (`git clone --no-hardlinks`) of `qcc-stage-playground` → `p01/ws/{claude,codex}`, branch `p01/<alias>` at `572572f` (= `8fe93fe` + a `.gitignore`-only commit); `origin` removed; `OPERATOR.md` removed by observer commit `97c9b59` (marked "not for participating agents") |
| Members | `codex` Codex 0.170.0, `gpt-6.2-sol`, effort low · `claude` Claude Code 2.1.332, `claude-sonnet-5-5`, effort low (both via Pastey "Model and effort", R0) — confirmed in Codex rollout `turn_context` and in every Claude transcript message's `model`; Claude effort not observable |
| Execution profile | "Unattended engineering" preset, **collaboration scope only** (no user default set). Codex: planning `read-only`, execution `workspace-write`, network off; Claude: `acceptEdits`, sandbox, push/PR/publish denied |
| Readiness (R2) | Both members ready before release (folder, installed, signed in, trust, model/effort, execution) |
| Plan-first (R5) | On; 3 planning turns/member (Pastey's fixed value = the brief) |
| Grants | 7 automatic turns / 2 h each (first turn is user-message-triggered and free) |
| Kickoff | One identical 1,179-char user message to each member, sent while Preparing: goal, budget, turn length, L=3 allowed / L=4 prohibited, subagents allowed, Pastey-only peer communication, protected files, no push. No roles, no hints |
| Role lines, preferences | Empty; no preference profile |

## Timeline (key points)

| Time | Basis | Event |
|---|---|---|
| 16:26:11 | pastey | Released; both first turns start together |
| 16:26:29 | pastey | claude proposes r1 (gadget-level track for itself, schedule track for codex) |
| 16:26:49 | pastey | codex proposes r2 (codex: retained-input stage from the explicit rational formulas; claude: independent arithmetic/qubit track) |
| 16:26:53 | pastey | claude accepts r2 → **r2 adopted** (all stances on r2, no structural problems) |
| 16:26:54 | pastey | codex, in a turn begun before adoption, accepts stale r1; no effect |
| 16:28:00 | pastey/verified | claude: GF(256) inversion via GF(16)/GF(4) norms, 36 T (vs 44), exhaustive component PASS; stage **873 T / 69 Q** |
| 16:28:01 | pastey/verified | codex: retained-input stage **230 T / 63 Q**, committed `a2bde2b` (R5 `done` artifact found) |
| 16:28:13 | pastey | claude declares `pastey-wait on=codex` (R3) for independent regeneration |
| 16:28:37 | pastey/verified | claude reproduces 230/63, combines with its gadget: **198 T / 61 Q** |
| 16:29:16 | pastey/verified | codex: live-norm stage **99 T / 71 Q** (`c29f6a2`); asks claude to send generator sources via Pastey (codex's clone has none of claude's files) |
| 16:29:44 | pastey | claude reproduces 99/71; **its message with the gadget source is dropped** (CF-1); Needs you = 1 |
| 16:30:21 | pastey/verified | codex: **99 T / 65 Q** (`cf9b1e4`) |
| 16:30:35 | pastey | claude reproduces 99/65 (hop 4 message: "nothing further is needed") |
| 16:30:46 | pastey | codex reports task done; collaboration quiescent from here |
| 16:37 | observer | After 5+ quiet minutes, observer ended the collaboration (Journal kept) |

## Research outcome

| Point (L = 3) | T | Q | Author | Generator | SHA-256 (prefix) | Reproduced by |
|---|---:|---:|---|---|---|---|
| incumbent | 953 | 71 | baseline | `baselines/e17_variants.py` | — | — |
| inv2 stage | 873 | 69 | claude | `work/claude/stage2.py 3` | `5ff71a2d` | observer only |
| retained-input | 230 | 63 | codex | `work/codex/retained_input.py` | `c9f31cee` | claude, observer |
| retained-input + inv2 | 198 | **61** | claude | edited copy of codex's generator | `7f584f38` | observer only |
| live-norm | 99 | 71 | codex | `work/codex/live_norm.py` | `698fc3b9` | claude, observer |
| live-norm compact | **99** | 65 | codex | `live_norm.py --compact --in-place` | `f222cb3b` | claude, observer |

Component: 36-T GF(256) inversion (`work/claude/gf256_inv2.kmx`, `b3950862`), exhaustive `check_component` PASS (16 bits, Q = 29).

Pareto front after P01: (99 T, 65 Q) and (198 T, 61 Q). Full hashes and commands: `observer-validation.md`.

Caveats [observer]:
- A 10× Toffoli cut in about 4 minutes of agent time is far larger than the playground's history suggests. The trusted verifier is exhaustive over the valid domain (16,646,400 of 16,777,216 inputs, as PROBLEM.md defines it). Before anyone treats the result as a research advance, a domain expert should confirm that this specification is the intended problem and that the construction's "retained input in linear coordinates" approach falls within it.
- Both generators are written for L = 3 (`spec.code(3)` and explicit L = 3 formulas). Nothing yet shows that the method generalises to L = 4. The post-experiment L = 4 check (20–35 min) has not been run.
- Claude's work is all uncommitted, and its 198/61 generator exists only as an edited copy of codex's file.

## Collaboration evaluation (separate from research success)

What worked:
- **Readiness gate and release (R2):** both started together; none of E01's start failures or trust problems occurred.
- **Plan-first (R5):** two proposals and one acceptance, adopted in 42 s. Pastey validated r2 with no problems. The roles were self-organised: codex proposed the split that was adopted.
- **Execution profile (R1):** read-only during planning, `workspace-write` during execution (rollout `turn_context`). No native approval was needed at any point.
- **Model and effort carried on every automatic turn (R0)**, as requested.
- **Fast results flowed through Pastey:** five delivered messages with SHA-256s and commands. Each member built on the other's result within one turn (claude's gadget to 198/61; codex's request for sources).
- **R3 wait:** claude declared one explicit wait on codex, and the wait ended when codex replied.
- **R5 artifact check:** each codex `done` artifact `p01/codex@<commit>` was verified as existing.
- **Guard rails held:** protected files unchanged in both clones (hashes before and after), selftest 33/33, no L=4 attempt (no `REFUSED` output in either transcript), no push, frozen playground and original research repository untouched.

Coordination failures and frictions:
- **CF-1, lost message (Pastey defect, important).** Claude's `pastey-message` contained a nested fenced code block (the inversion source). The inner fence ended the block early, the rest counted as "not at the end of the reply", and Pastey dropped it. All it recorded was an attention line ("Needs you 1"). Claude's summary then said "Sent codex my inversion gadget source", a false belief no one corrected. Codex had asked for exactly this source in order to reproduce claude's results. Effect: **codex never reproduced claude's 873/69 or 198/61.** Possible fixes: let message bodies contain fences (longer outer fence or a length-prefixed block), tell the sender in its next prompt that its block was dropped, and teach the note not to use nested fences.
- **Asymmetric reproduction channel.** Claude reproduced codex's work by *reading codex's folder* and copying its generators. The kickoff forbade editing the peer's folder, not reading it. Codex kept to Pastey and had no way to get claude's files. In practice R5's `artifact=<branch>@<commit>` is only checkable in the owner's own repository. With independent clones there is no Pastey-mediated artifact handoff, so reproduction depends on filesystem access outside Pastey.
- **Stale acceptance race.** Codex's planning turn accepted r1 after r2 had already been adopted. It had no effect, but it shows that acceptances in flight are not reconciled with adoption.
- **Grant accounting vs. the brief.** The "Shared plan adopted" message is delivered under each member's grant. With a 7-turn grant, each member could get up to 6 execution turns, not 5. The observer's attempt to withdraw and re-grant was refused by the observer's own permission check, so I recorded this as a deviation. Actual execution turns: claude 5, codex 4. Planning turns: 2 each. Neither cap was reached.
- **Early quiescence is invisible.** When the work stopped at 16:30:46, nothing told the user the collaboration was idle and effectively finished. The only Needs-you item was CF-1.
- **No subagents** were used; it was permitted but unnecessary at this turn length.

Measures:

| Measure | claude | codex |
|---|---:|---:|
| Turns (planning / execution) | 2 / 5 | 2 / 4 |
| Longest turn (time since its previous Journal event) | ~67 s | ~75 s |
| Messages sent (delivered) | 4 (3) + 1 dropped | 3 + 1 r1 acceptance |
| Tool calls | 20 (19 Bash, 1 Write) | 18 |
| Tokens | in 60 + cache-create 80.8k + cache-read 1.47M; out 16.8k | in 1.16M (1.11M cached); out 9.7k (1.5k reasoning) |
| Commits | 0 (all untracked) | 3 (`a2bde2b`, `c29f6a2`, `cf9b1e4`) |
| Full-tier stage results claimed / observer-verified | 2 / 2 | 3 / 3 |
| Human interventions after release | 0 | 0 |

Time to first delivered peer message: 18 s (claude's r1 at 16:26:29). Time to adopted plan: 42 s. Time to first verified improvement: 1 min 49 s.

## Deviations from the brief

- Baseline commit `572572f` instead of `8fe93fe` (`.gitignore` only); `OPERATOR.md` withheld from participants.
- Selftest reports 33 checks, not 36 (the README also says 33).
- Up to 6 execution turns were possible per member (see above); at most 5 were used.
- The collaboration ended after about 11 minutes (4.6 active), not 60. The agents finished on their own and the observer ended it.
- Full-screen control was used for the native folder picker, the dropdowns, and End.

## Files

- `raw/journal-export.jsonl`: the full Journal (54 entries + end), `raw/collaboration-record.json`
- `observer-validation.md`, `observations.md`, `setup-log.md`
- `raw/worktrees/{claude,codex}-work.tgz`: each member's `work/` and ledger; `*-commits.txt`, `*-status.txt`
- Kept outside this repository, in `~/research-playgrounds/p01/evidence/raw/runtime/`: the Claude transcript and the Codex rollout (they contain account identifiers), plus `resources.log` (30 s process samples)
