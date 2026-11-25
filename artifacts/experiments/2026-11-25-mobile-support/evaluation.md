# Evaluation: Pastey as a governance layer (2026-11-25)

Independent assessment by the observer. Every claim points to `timeline.md`, `metrics.json` or files under `raw/`. "Defect" means Pastey behaviour that contradicts its own design doc (`docs/research/agent-collaboration-links.md`) or that left the user unable to govern; "limitation" means a native runtime constraint Pastey inherits.

## Bottom line

The experiment produced **no inter-Agent collaboration at all**: 0 messages delivered between Agents, 0 automatic turns, out of 40 granted. That was not because the Agents behaved badly. Both read the repository well and independently reached a near-identical product interpretation. Pastey failed at three points that its design says it handles:

1. **A member blocked on a native approval looked "Working" for 47 minutes, and no screen let the user see or answer the approval.** This alone halted the collaboration (`timeline` 16:00, 16:08).
2. **A Claude Code member could not start in a git worktree** of an already-trusted repository; the reason never reached the UI (`timeline` 15:09).
3. **Automatic turns run without the model, effort and permissions the user chose**, so a meaningful configuration needed hand-edited native files before anything could run (`preflight.md` §1–3).

The concurrency model (one turn boundary per member, delivery only at that boundary) turns any one of these into a stall for every member.

## Findings, prioritised

### P0 — Native approvals are invisible to the collaboration (governance defect)

- **Evidence.** codex's rollout ends at 15:21:51 with an `exec_command` carrying `sandbox_permissions: "require_escalated"`. The codex card kept showing "Working · Turn running" up to 60 m; Needs you counted only claude's question. `collaboration.rs` and `CollaborationScreens.tsx` contain no native-approval handling; `NativeDecisionPanel` renders only from a Bridge's Agents task tracker. "This device" listed no folders, so the request was unreachable. The member menu offered only "Edit role…" (no Stop). Ending the collaboration did not stop the turn; only quitting Pastey did.
- **Design says** (Journal entries table and "What happens when the recipient is…"): an `attention` entry for "a native approval waiting", shown under Needs you, which is "the only surface that interrupts".
- **Consequence.** The `auto` grant was useless: the scheduler waits for a turn boundary that never comes, and claude's message waited behind it. The user saw activity, not a block.

### P1 — Start failures lose their reason; a worktree defeats the trust check

- **Evidence.** Three `native_agent_start_failed` turns in the same second, then "undeliverable: the member's turn could not start". `trust_problem` has a specific, actionable message ("Open the folder in Claude Code once … accept its trust prompt"), but neither the Journal nor Needs you showed it (Needs you stayed 0, despite the design's "after three such attempts it becomes *needs you*"). The suggested remedy did not work: Claude Code showed **no** trust prompt in the worktree and wrote no record, apparently inheriting trust from the main repository. Pastey's check looks only at the worktree path and the root implied by its `.git` *file*. The only way through was to hand-edit `~/.claude.json` (I-3).
- **Also:** three attempts in the same instant are not retries; they cannot succeed and only fill the Journal.

### P1 — Automatic turns ignore the conversation's model, effort and permission intent

- **Evidence.** `collaboration::drive` passes `NativeSessionOptionsV1::default()`. Claude Code keeps a model across `--resume` but not an effort, so every automatic Claude turn would run at the CLI default. Headless `-p` with no permission mode declines edits and shell commands, so a Claude member cannot build or test anything. Even with `acceptEdits` (I-1), claude's transcript shows `cargo`, `npm`, `rustc`, `xcodebuild` declined.
- **Consequence.** The two members were never equals. Codex (sandboxed, but able to run `cargo`/`npm` offline) could verify; Claude could only write unverified source and ask others to compile it. Claude assumed codex was equally limited, which was wrong, and nothing in Pastey told either of them what the other could do.

### P2 — Long first turns plus boundary-only delivery give the Agents no chance to coordinate early

- claude's first turn lasted 17 min; codex's was still running after 60. Both did the same crate split and both created `lib.rs` and `capabilities/mobile.json` before either could read the other (`outcome.md`, overlap list).
- claude's request "do not redo the lib.rs/main.rs split" was written after codex had already done it. By design, nothing reaches an Agent mid-turn. With open-ended objectives, the first turn is where all the duplicated work happens.
- The note asks for no acknowledgements and few messages, and claude complied: one useful message to codex, one to the user, no acknowledgements. Behaviour was not the problem; timing was.

### P2 — The user-facing levers are hard to reach and slightly misleading

- Background computer use could not drive the native folder picker or the grant dropdowns. Full-screen control was needed four times. This is partly a tooling limit, but it means no part of a collaboration's setup can be scripted or automated today.
- "Confirm decisions" made one Agent's architecture the user's decision while the other Agent was still mid-turn (H-1). Pastey shows such confirmations as the user's in digests and briefings, which is powerful steering. The UI gives no hint that another member is still working on the same question.
- After End, both members showed "asks you again" while codex kept running unseen.

### Effective behaviours (Agents)

- Independent, evidence-based interpretation; options weighed (claude: requester peer vs browser remote UI vs phone as Host). Honest uncertainty: claude marked platform facts "to verify" and wrote "nothing compiled".
- Correct use of Pastey's protocol: addressed messages, a structured summary with `decisions`, product questions sent to `user` instead of being decided silently. No acknowledgement churn.
- codex validated where it could (offline check and build) and asked for an escalation instead of working around the sandbox.

### Ineffective behaviours (Agents)

- Both started with very large first turns (claude: 383k output tokens, 62.6 M cache reads; codex: 5.7 M input tokens) before any exchange. Under boundary-only delivery this maximises duplicated work.
- claude asked the user to run builds by hand, which pushes coordination work back onto the human the system is meant to relieve.
- claude's doc changes touched unrelated files (`--bin` → `--lib` across docs and a Windows script) as part of an unverified change.

## Human coordination required

| | Count | What |
| --- | --- | --- |
| Setup decisions by the user | 4 | Claude config, permissions, approval relay policy, launch |
| Native trust actions | 2 | Running `claude` in the worktree (no effect), authorising I-3 |
| Full-screen control approvals | 4 | Folder picker ×2 sessions, grant dropdowns, teardown menus |
| Product decisions | 1 | H-1, confirmed claude's decisions |
| Agent questions to the user | 1 | Left unanswered by choice |
| Native approvals answered | 0 of 1 | Never surfaced |

**Net effect [observer]:** human coordination *increased* compared with running two separate Agent sessions by hand. Every unblock went through the user, and the main blocker could not be found from Pastey's UI.

## Governance defects vs native limitations

| Issue | Classification |
| --- | --- |
| Approval not surfaced; member shown "Working" | Pastey defect (contradicts design) |
| No Stop on a running member turn; End does not stop it | Pastey defect / UX gap |
| Start-failure reason not shown; 3 instant retries; Needs you not raised | Pastey defect (contradicts design) |
| Trust check doesn't recognise worktrees of a trusted repo | Pastey defect, triggered by Claude Code's undocumented trust inheritance |
| Automatic turns drop model and effort | Pastey limitation, by design |
| Headless Claude declines commands | Native limitation (Claude Code `-p`) that Pastey exposes without explanation |
| Codex sandbox has no network; escalation needs approval | Native limitation, correctly respected |
| Claude Code effort not observable | Native limitation |
| Folder picker / dropdowns need full-screen control | Tooling friction (computer use) |

## Recommendations (ordered by evidence weight)

1. **Turn every native approval of a member's running turn into a Needs you item with Accept and Decline in the collaboration view**, and change the member card to "Waiting for your approval: <command> (since 15:21)". Write the design's `attention` entry. Without this, `auto` grants cannot work for any non-trivial task. (P0)
2. **Add Stop to a member's running turn in the collaboration view, and make End ask its running turns to stop**, as the design already says for Burn. (P0)
3. **Carry the start failure's reason into the Journal and Needs you**, and stop after the first definite refusal instead of three instant attempts. For the trust case, say exactly what Claude Code needs. (P1)
4. **Make the trust check worktree-aware**: resolve `git rev-parse --git-common-dir`, or accept trust recorded for the main checkout. Then measure Claude Code's own rule; this run suggests it inherits trust without writing a record. (P1)
5. **Store the user's model and effort per member, and pass them on every collaboration turn**, including automatic ones. Show the effective model, effort and permission mode on the member card. (P1)
6. **Tell the user, when adding a member, what it will be able to do headless** (Claude `-p` declines commands; Codex has no network). Put a one-line capability statement per member into the collaboration note, so Agents don't guess each other's limits. claude guessed wrong here. (P1)
7. **Bound the length of a first turn or offer an early boundary.** For example, a note line asking members to publish a plan and claimed files within their first few minutes and end the turn, or a "first turn: plan only" option on the add dialog. This is the only way to reduce duplicated work under boundary-only delivery. (P2)
8. **Show a "member still working" warning next to Confirm decisions** when another member is mid-turn on the same collaboration. (P2)
9. **Make collaboration setup scriptable**: a CLI or test-harness entry that drives the real app's commands, unlike `ui-bridge`, which forces effort `low`. Then experiments like this need no screen control. (P2)

## What to consider next, given this blocker

- **Re-run conditions.** Before another open-ended run: fix 1–3 above, or pre-approve the escalation class Codex is likely to request; decide whether Claude members may run builds (an allow-list such as `Bash(cargo *)`, `Bash(npm run *)`), and give both members symmetric abilities. Otherwise the run measures permission asymmetry, not collaboration.
- **Network for builds.** Codex's sandbox has no network, so `npm ci`/`cargo fetch` work only from warm caches. Mobile targets (`rustup target add`, Xcode, Android SDK) are absent on this Mac. Either provision them or tell both Agents the run is source-only.
- **A watchdog for silent stalls.** A member "Working" with no native event for N minutes should become a Needs you item. Pastey already receives the events it would need, so this is still passive observation.
- **Decide who settles architecture.** Here the user did it (H-1) before the Agents exchanged anything. If Agent negotiation is the subject of study, product-decision confirmations should be held back until both members have had a turn, or the UI should make the timing visible.
- **The objective itself is viable.** Both Agents independently found the same requester-only design and a credible milestone path (`outcome.md`). A follow-up run is worth doing once the governance gaps above are closed, preferably with both branches based on one agreed crate split.
