# Unattended collaboration: assessment after E01 (2026-11-25)

Status: assessment written on 2026-11-25; the user's decisions on it are recorded the same day in [Decisions](#decisions-2026-11-25), which supersede the assessment where they differ, and implementation is authorised in the order of the [revised roadmap](#revised-roadmap-and-experiments). The assessment reviews seven design intentions the user wrote after E01 against the E01 evidence, Pastey's code at `cf77405`, the installed native runtimes and their current official documentation, and standard coordination mechanisms.

## Decisions (2026-11-25)

The user accepted the direction and decided:

1. **Persistent default execution profile.** Authorising unattended execution stays explicit and informed, but once: a **User Default Execution Profile**, then **collaboration overrides**, then **member overrides**, enforced by the **native runtime**. New collaborations inherit the default. The effective configuration is visible, editable and revocable. Native permission modes, approval reviewers, sandbox policies and execution settings are used; Pastey builds no approval system of its own. Configured permissions are shown apart from verified runtime capabilities, and native auto-review is never presented as a deterministic security boundary.
2. **Notifications.** Local-first does not mean local-only, and notification delivery is separate from execution authority. Optional channels may include local OS notifications, paired devices, mobile push, email and SMS. External channels carry no authority, and sensitive content stays local unless the user permits its disclosure. Approvals stay authenticated, bound to the exact request, and go through Pastey's existing authority path. Start with the simplest useful channel, with no mandatory cloud service. (This supersedes the assessment's "conflicts with local-first" in [§3](#3-escalation-and-remote-notification).)
3. **Readiness and liveness** proceed as proposed. Every required member is ready before the objective is released. Verified capabilities and limits are shown to the other members, labelled **configured, probed, measured or unknown**. The watchdog distinguishes inactivity, native approval waits, dependency waits, provider failures and unknown outcomes; silence alone is never failure. Running members have their own Stop with an observable cancellation outcome.
4. **Planning must converge.** A fixed planning budget is not agreement. The lifecycle is: independent planning; exchange of plans and capability constraints; negotiation and convergence; a shared plan; mechanical validation by Pastey; explicit commitments to responsibilities and deliverables; asynchronous implementation. Pastey detects overlapping claims, inconsistent dependencies, resource conflicts and authorisation problems; Agents decide the solution, responsibilities, interfaces and strategy. No predefined roles and no intelligent project manager. If the planning budget runs out without convergence, the collaboration **pauses** (or uses an explicitly authorised conflict-resolution mechanism); conflicting plans are never released. The representation of plans and commitments stays minimal, not a project-management system. (This supersedes the assessment's "release when K turns are used" in [§5](#5-planning-phase).)
5. **Asynchronous dependencies and commitments**: ownership, commitments, dependency declaration and satisfaction, artifact handoff, cycle detection (logical waits can deadlock a collaboration even when nobody holds a resource), deadlines, progress monitoring, failure and replanning. Deterministic and minimal; no scheduler beyond what the evidence justifies.
6. **The preference profile moves before plan-first**, because preferences shape how Agents read the objective and form plans. Pastey stores, versions and delivers it; Agents interpret it. It separates hard restrictions, delegated decision scope, ordinary preferences and previously confirmed decisions, and never creates execution authority. Routine reversible decisions within scope are made autonomously; escalation is for material uncertainty, substantial risk or decisions outside scope.
7. **Invariants.** Pastey stays governance infrastructure: native Agents own HOW (reasoning, planning, tools, native permissions, sessions, strategies); Pastey owns governance, authority, readiness, coordination, dependencies, eligibility, observability and lifecycle. No proprietary harness, mandatory LLM orchestrator, intelligent project manager, general workflow engine or unnecessary cloud dependency. Existing non-goals are amended explicitly where E01 justifies it (here: "Pastey passes only the folder" and "not a task board"); all other invariants stay. No release or remote push without explicit authorisation.

## Evidence labels

- **[E01]**: the evidence package `artifacts/experiments/2026-11-25-mobile-support/` (timeline, evaluation, Journal export).
- **[repo]**: Pastey code and docs at `cf77405`, named by file.
- **[installed]**: measured on this Mac. Codex 0.170.0, from `codex app-server generate-json-schema`. Claude Code 2.1.318, from `claude --help`.
- **[docs]**: official documentation read on 2026-11-25 (sources at the end). Some pages were read only in part and are marked where that matters.
- **[assumed]**: not measured.

## Summary

- **Most of what the user asks for already exists in the native runtimes. E01 used none of it.** Pastey started members with no permission mode, no sandbox profile, no approval reviewer and no model or effort on automatic turns ([repo] `collaboration::drive`, `claude_code/conversation.rs`). Both runtimes now have native unattended mechanisms:
  - **Claude Code:** `--permission-mode auto`, a background classifier ([installed], [docs]); allow and deny rules; sandboxed Bash with a domain allow-list.
  - **Codex:** `approvalsReviewer: "auto_review"`, a reviewer subagent that approves or denies escalations by risk ([installed] schema); `sandboxPolicy.workspaceWrite.networkAccess`; execpolicy rules.

  Both can be set per run or per turn. Pastey needs to *carry* a user-chosen native execution profile, not build a permission system.
- **E01's stall was a Pastey defect, not missing autonomy.** A native approval was waiting and Pastey showed the member as "Working" ([E01] timeline 16:00). Codex even exposes this state as `thread/status/changed` with `activeFlags: ["waitingOnApproval"]` ([installed]). Fixing visibility, stop and readiness comes before any new concept.
- **Five new primitives are justified by the evidence.** Each stays within Layer 5's boundary as an extension of rules that already exist:
  1. a per-member execution profile;
  2. a readiness barrier;
  3. declared waits with deadlines;
  4. a stall watchdog;
  5. Agent-authored claims and dependencies.

  A full shared task graph, remote push, email and SMS, and a Pastey-evaluated preference engine are **not** justified yet, or conflict with existing principles. See [Challenges](#challenges-to-the-proposals).
- **"Unattended by default" should be a preset the user picks, not an implicit default.** The invariants require informed, local consent for stored authority. A model-based auto-reviewer is a judgement, not a boundary. The boundary must stay the native sandbox, the network allow-list and the deny rules.

## What E01 actually demonstrated

| # | Deficiency | Evidence | Kind |
| --- | --- | --- | --- |
| D1 | A native approval blocked a member for 47 min and was shown nowhere; the member read "Working" | [E01] timeline 15:21:51–16:09; `CollaborationScreens.tsx` renders no `NativeDecisionReviewV1` | Pastey defect |
| D2 | No Stop for a running member turn; End left it running | [E01] timeline 16:08–16:09 | Pastey defect |
| D3 | A start refusal lost its reason; 3 instant attempts; Needs you stayed 0 | [E01] seq 8–10; `trust_problem` text not in the Journal | Pastey defect |
| D4 | The trust check rejected a git worktree of a trusted repo, and Claude Code offered no prompt to fix it | [E01] 15:09–15:13, I-3 | Pastey defect plus an undocumented native rule (docs: trust is in `~/.claude.json`; no worktree rule found) |
| D5 | Automatic turns pass no model or effort | [repo] `turn_options()` returns default; [E01] I-1 | Pastey limitation |
| D6 | The Claude member could not run `cargo` or `npm`; Codex had no network | [E01] transcript (declines), rollout (`network_access=false`) | Native defaults that Pastey neither configured nor disclosed |
| D7 | Members guessed each other's abilities wrongly | [E01] seq 13–14: "neither I nor, as far as I know, codex can run cargo" | Missing facts, not missing intelligence |
| D8 | Both members made the same crate split and created the same files before any exchange | [E01] `outcome.md` overlap list | Coordination timing |
| D9 | The user decided architecture mid-run, before the second member saw it (H-1) | [E01] seq 16 | Governance UX |
| D10 | Setup needed full-screen control four times | [E01] config | Tooling (UI not scriptable) |

**Not demonstrated by E01:** deadlock, livelock, message storms, artifact handoff, replanning, preference conflicts, or remote notification needs beyond D1. These remain design risks, not observed failures. The roadmap treats them that way.

## What Pastey already satisfies

| Intention | Already there | Where |
| --- | --- | --- |
| Bounded unattended turns | Auto grant: N turns ≤ 20, ≤ 24 h, ≤ 30 per hour per device; stored, revocable | [repo] `collaboration.rs`, Layer 5 |
| Async execution without holding a turn | Members end their turn; delivery happens at the next boundary; at most one scheduled turn per member | [repo] design "Scheduling and the turn boundary" |
| Escalation channel | Messages to `user` go to Needs you; summaries with `decisions`; Confirm and Reject | [repo] |
| Record of decisions | Journal `user` and `summary` entries with basis labels; briefings carry confirmed decisions | [repo] |
| Loop bounds | Hop limit 4, ≤ 3 messages per turn, ≤ 20 waiting per member | [repo] |
| Approvals bound to the exact action | `NativeDecisionReviewV1`: BLAKE3 digest of the exact content, one-time `accept`/`decline`, device-key-bound remote answers, 30 min expiry | [repo] Layer 5 "Native approval requests" |
| Fixed capability probes | `runtime.rust_cargo`, `runtime.node`, `runtime.git` …, never authority | [repo] architecture "Host and capability observations" |
| Pre-launch checks per adapter | Trust, workspace in use, model and effort against the native catalog (`codex_options_problem`) | [repo] |
| Progress without model calls | `activity` from native events | [repo] M2 |

## What the native runtimes already provide

### Unattended execution and permissions

| Need | Claude Code 2.1.318 | Codex 0.170.0 |
| --- | --- | --- |
| Run without routine prompts | `--permission-mode auto`: a classifier reviews actions before they run [installed][docs]. `acceptEdits` covers edits only. `dontAsk` denies what would prompt. | `approvalPolicy: "on-request"` + `approvalsReviewer: "auto_review"`: a reviewer subagent decides escalations [installed]. `never` turns prompts off and leaves the sandbox as the only limit [docs]. |
| Pre-authorise exact commands | Allow rules such as `Bash(npm run *)`; deny, then ask, then allow; per run via `--settings` or `--allowedTools` [docs][installed] | execpolicy rules (allow, prompt, forbidden) [docs, syntax page not read]; `acceptWithExecpolicyAmendment` [installed] |
| Technical boundary | Sandboxed Bash (macOS, Linux, WSL2): writes to the working directory; network through a proxy with `sandbox.network.allowedDomains`; `autoAllowBashIfSandboxed` [docs] | `sandboxPolicy` `workspaceWrite` with `writableRoots` and `networkAccess`; experimental `features.network_proxy.domains` [installed][docs] |
| Set per run or turn | `--permission-mode`, `--settings '{…}'`, `--model`, `--effort` on each `-p` run [installed] | `turn/start`: `approvalPolicy`, `approvalsReviewer`, `sandboxPolicy`, `model`, `effort`; turn overrides persist for the thread [installed][docs] |
| Hard restrictions | Deny rules, evaluated first [docs]; protected paths still prompt under `bypassPermissions` [docs] | `forbidden` rules; `.git` read-only in `workspaceWrite` [docs] |
| An external answerer | `PermissionRequest` hook (allow or deny, `updatedInput`); `--permission-prompt-tool`; `--permission-prompts host\|none` [installed][docs] | Server-to-client requests answered by the client (Pastey already relays them) [repo][installed] |
| Signal that it is waiting | `Notification` hook matchers `permission_prompt`, `agent_needs_input` [docs] | `thread/status/changed` `active` with `waitingOnApproval` or `waitingOnUserInput` [installed]; `notify` on `agent-turn-complete` [docs] |
| Readiness without a model call | none documented; Pastey's own checks (CLI, trust, catalog) | `account/read`, `account/rateLimits/read`, `config/read`, `command/exec` with a `sandboxPolicy` [installed] |
| Planning | Plan mode, also with `-p` (`--permission-mode plan`); `TodoWrite` [docs][repo] | `turn/plan/updated` (step, status); experimental `collaborationMode`; `thread/goal/set` with `tokenBudget` [installed][docs] |

### Multi-agent features, and why they don't replace Pastey

- **Claude Code agent teams** [docs] have a lead, teammates, a mailbox, and a shared task list with three states, dependencies, and claiming under file locking. Teammates' approvals bubble up to the lead, and teammates are not isolated in worktrees. But they are experimental, Claude-only ("the workers are Claude sessions"), interactive-only ("Claude doesn't spawn teammates" in `-p` mode), and not resumable.
  - **Useful as evidence:** a lightweight claimable task list with dependencies is enough for Anthropic's own multi-session coordination. Its documented failure mode is file conflicts, and the documented remedy is file ownership.
- **Claude Code cross-session messaging** [docs] works between Claude sessions only, delivers mid-turn or starts a turn when idle, and offers `notify_when_idle`. Collaboration members refuse it, as decided in M1.
- **Codex subagents** [docs] are spawned within one Codex session. They share neither a provider-neutral task state nor a Journal.

**Conclusion:** nothing native coordinates *across providers or devices*, or records provenance. Execution, permission, planning-within-an-Agent and waiting signals are native. Pastey should adopt them rather than duplicate them.

## Challenges to the proposals

### 1. "Unattended collaboration should be the default"

- **Agree** that routine reading, editing, building, testing and fetching dependencies must not need a human.
- **Disagree** that Pastey should pre-authorise them as an implicit default:
  - **Consent.** Layer 5's invariants make every stored authority a local, informed decision of the executing Host's user (Full Access, the auto grant). An execution profile is a third stored admission and should follow the same rule. The create dialog can offer "Unattended engineering" as its first, pre-selected preset, which gives the same effect with visible consent.
  - **Classifiers are not boundaries.** Claude's auto mode and Codex's `auto_review` are model judgements. In E01, the action that stalled was a request to run a headless browser outside the sandbox ([E01] `codex-escalated-browser-check.mjs`). That is exactly what an auto-reviewer might approve. The enforceable boundary is:
    - the native sandbox (writes confined to the member's folder);
    - a network allow-list (package registries, documentation hosts);
    - native deny rules (push, publish, release, credentials).
  - **"Obtaining dependencies" is the riskiest routine activity.** Prefer locked installs (`npm ci`, `cargo --locked`) through an allow-listed registry. Fetching arbitrary URLs is a supply-chain and exfiltration path.
- **Pastey never answers a native approval itself.** This stays: a native reviewer chosen by the user is the runtime answering its own approvals, which the invariants allow. Anything that is still escalated after the native reviewer goes to the user (D1).

### 2. A preference profile

- **Mostly existing primitives.** Goal, roles and confirmed decisions are already `user` entries carried by the note and briefing. Agents already read `CLAUDE.md` and `AGENTS.md`. A profile is a longer, versioned `user` entry: product and engineering preferences, platform preferences, risk tolerance, delegated scope, restrictions.
- **Pastey must not evaluate it.** "Reversible" and "within delegated scope" are judgements. If Pastey decides them, it becomes the intelligent orchestrator the user wants to avoid. Pastey stores, versions and delivers the profile, and records each Agent decision it was used for as an Agent statement.
- **Restrictions must be enforced natively where they can be.** "Never push", "never touch `main`" or "no network except registries" go into the execution profile as deny rules and sandbox settings. A text-only restriction is advice, and the profile UI should say so.
- **Risks:**
  - **Size:** the briefing is bounded at 12 KiB and the note travels on every turn.
  - **Staleness:** a decision made in another project leaks into this one.
  - **Injection:** another Agent's quoted text is not the profile.

  Keep it per collaboration, with an optional user-level template copied in at creation.
- **E01 nuance:** claude's question to the user (D9) was a reasonable escalation. Changing the crate type rewrites every test command. A profile line such as "build-system changes are within delegated scope; record the reason" would have let it proceed. "Which phone first" is a real product preference that a profile can answer.

### 3. Escalation and remote notification

- **Desktop notifications are cheap and fully in scope:** a local OS notification on a new Needs you item.
- **Remote decisions already have the right mechanism.** A paired device can answer a native approval of a task it requested, bound to the request's digest and the device key ([repo] Layer 5). A paired phone (the mobile work) answering a collaboration Needs you item is an extension of that path. It needs the phone's app to be open, since the mobile design is foreground only.
- **Mobile push, email and SMS conflict with Pastey's local-first design.** They need a server or third-party credentials, and Layer 4 lists push and offline delivery as out of scope. If added, they may carry only "something needs you on <device>", never decision content or authority. A user-supplied `notify` command, as Codex does, keeps the egress the user's own choice.
- **"Only the affected task waits"** needs to know what depends on what. Messages alone don't say that. See intention 6.

### 4. Readiness barrier: strongly supported (D3, D4, D5, D6, D7)

Per member, all without model calls:
- **CLI:** present and supported version.
- **Sign-in and quota:** Codex `account/read` and `account/rateLimits/read`; Claude via its CLI status, to verify.
- **Trust:** worktree-aware.
- **Model and effort:** in the native catalog, and stored for every turn.
- **Execution profile:** resolves to native flags.
- **Fixed probes:** for the tools the objective needs, such as `runtime.rust_cargo` and `runtime.node`.
- **Sandbox check:** for Codex, a `command/exec` of `cargo --version` under the member's own `sandboxPolicy` shows whether the sandbox lets it build.
- **Communication readiness:** M0 and M1a measured block reliability, so a recorded, versioned measurement suffices.

Two caveats:
- **Readiness is a snapshot.** A sign-in expires or a quota runs out mid-run. Fast-failure pausing (M2) must stay.
- **"Verified facts" must be labelled by how they were obtained:**
  - "configured" — the profile says so;
  - "probed" — `command/exec` ran it;
  - "measured" — an earlier turn did it.

  Claude Code offers no model-free sandbox probe [docs: none found], so most Claude facts are "configured".

The facts go into the collaboration note as Pastey facts, for example "codex: network to registry.npmjs.org, crates.io; can run cargo (probed)". This directly fixes D7.

### 5. Planning phase

- **Agree** that a published plan before implementation addresses D8.
- **Disagree** with an open-ended negotiation phase. Under boundary-only delivery each exchange costs a full turn per member. Two capable Agents can plan indefinitely (a livelock), and Pastey cannot judge when a plan is "good enough" without becoming the orchestrator.
- **Minimal form:**
  - Each member's first turn runs in the native plan mode: Claude `--permission-mode plan`; Codex read-only sandbox, or `collaborationMode` once stable.
  - It must end with a **plan block**: understanding, proposed stages, the claims it wants (stages and path globs), dependencies, deliverables, and how its work will be integrated.
  - Pastey merges the blocks deterministically and flags overlapping claims and dependency cycles.
  - Members resolve conflicts by messages, bounded to K planning turns (for example 2).
  - Implementation turns are released when there is no open conflict, or when the K turns are used (then Needs you).

  Pastey decides nothing technical. It checks only structural facts: overlap, cycles, unclaimed dependencies.
- **Conflict with an existing principle:** the collaboration design says "not a task board or project-management system" ([repo] design, "What this is not"). E01's duplicated work (D8) is the evidence for crossing that line a little. The design should be amended explicitly, and the crossing kept to claims, dependencies and states, all Agent-authored. Assignment, priority, estimation and progress percentages stay out.

### 6. Asynchronous execution and dependencies

**The turn-boundary model already avoids classic deadlock.** A waiting member ends its turn and holds nothing. The real risks are **liveness**: waiting forever for a message, an approval (D1) or an artifact. Mechanisms, cheapest first:

| Risk | Mechanism | Precedent |
| --- | --- | --- |
| Silent native block (D1) | Map `waitingOnApproval` / `waitingOnUserInput` (Codex) and pending reviews (both) to Needs you; a watchdog when no native event arrives for N minutes | Heartbeats and failure detectors |
| Waiting for something that never comes | **Declared waits with a deadline**: a block `pastey-wait on=<member>/<claim>/<message>/user until=<duration>`. At the deadline Pastey wakes the waiter with "deadline passed" and tells the user. | Leases and timeouts (Gray & Cheriton); futures with timeouts |
| Static dependency cycles | Reject a claim whose dependency would close a cycle (topological check on insert) | DAG validation (Kahn) |
| Runtime circular waiting | A wait-for graph over declared waits; a cycle is flagged to every member in it, never broken by Pastey | Wait-for graph detection (Chandy–Misra–Haas) |
| Livelock: messages without progress | A progress measure from facts Pastey already has (claim state changes, files changed, tests reported); after N automatic turns with messages but no progress, pause and send to Needs you. The hop limit stays as a backstop. | Progress-based livelock detection; bounded retries |
| Contention for files | Path claims checked against each turn's manifest; a turn that changed another member's claimed path is flagged, not reverted | Advisory locking; Claude's own "file ownership" guidance |
| Failure and cancellation | Claim states `failed` and `released`; waiters on a released claim are woken | Saga-style compensation is left to Agents |
| Artifact handoff between worktrees | An artifact is a commit on a named branch. Pastey records "claude published `exp/x@<sha>`" as a fact after checking the commit exists in the shared object store; across devices, the existing Transfer or Movement. No file sync. | Content-addressed references |

Pastey stays mechanical here: every rule is a check on declared structure or observed facts, never a judgement about content.

### 7. Architectural boundary

| New primitive | Boundary check |
| --- | --- |
| Execution profile per member | A third stored admission, beside Full Access and the auto grant. It needs the same rules: created only by a local command, informed consent, never widened by a peer, revocable. Pastey passes native flags and never answers approvals. **Changes** the current "Pastey passes only the folder" rule ([repo] Layer 5 Full Access, `conversation.rs` header): record that change explicitly. |
| Readiness barrier | Observation only; "availability is never permission" holds. |
| Plan, claim and wait blocks | Extends the existing exception "a captured final reply is read for fenced blocks, only in a collaboration's sessions". Same bounds, faithful-rendering check, Agent-statement basis. |
| Watchdog, deadlines, wait-for graph | Pastey facts from events it already receives; no model calls, no mid-turn injection. |
| Preference profile | A `user` entry; content delivered, never interpreted. |
| Notifications | Local OS notifications are in scope. Remote push is out of scope for Layer 4 unless the user accepts the local-first trade-off. |

**Everything fits Layer 5** if two documented principles are amended:
- "Pastey passes only the folder";
- "not a task board".

Neither needs an LLM-dependent component. The one model-based step, approving escalations, stays inside the native runtimes, which the user chose.

## Failure modes to design for

- **Execution profile drift.** Native flags change between versions; `on-failure` was deprecated and `untrusted` retired, per [docs]. Pin the profile to the measured CLI version, and refuse or warn on unknown versions as the readiness barrier.
- **Auto-reviewer approves too much.** Keep deny rules for irreversible outward actions independent of the reviewer, and record every auto-approved escalation as a runtime report in the Journal where the native event exposes it. Codex exposes guardian assessments (`thread/approveGuardianDeniedAction` exists) [installed]; Claude has `PermissionDenied` hooks for auto mode [docs].
- **Quota exhaustion mid-run.** Both members draw on the user's subscriptions. E01 used 5.7 M Codex tokens and 62.6 M Claude cache reads in one turn each. Add a usage line to readiness, and pause members on quota errors (M2 already does this).
- **A plan phase that ends badly.** K planning turns used with conflicts still open goes to Needs you, which is attended. Make K small, and let a profile line delegate tie-breaks ("on overlapping claims, the earlier claim keeps it").
- **Claims as a single point of contention.** Path globs too broad ("src/**") block everything. Bound the claim count and size, and show contention.
- **Two worktrees, two branches, one integration.** Somebody must merge. Make "integration" an explicit stage in the plan block, owned by a member. Pastey does not merge.
- **Readiness false positives.** "Configured" is not "works": E01's Claude member was configured `acceptEdits` and still could not run `cargo`. Show the label.

## Minimal roadmap to E02 (superseded)

The assessment's first roadmap (R0 fixes, R1 profile, R2 readiness, R3 liveness, R4 plan-first, R5 preferences; remote channels left out) is replaced by the revised roadmap below. It is kept in the history of this file.

## Revised roadmap and experiments

Decided on 2026-11-25. Each milestone sets its exact boundary, inspects existing code before adding abstractions, checks native interfaces against the installed versions, implements and tests the smallest complete solution, keeps authority and lifecycle guarantees, and records evidence and limits ([Progress](#progress)).

| Step | Scope | Fixes |
| --- | --- | --- |
| **R0: Correctness and lifecycle closure** | Native approval visibility; Needs you integration; member-level Stop and observable cancellation; accurate execution states; start-failure diagnostics; worktree-aware trust; model and effort kept across automatic turns | D1–D5 |
| **R1: Execution profile** | Persistent user default; collaboration and member overrides; native permission and sandbox configuration; effective configuration shown; explicit authorisation and revocation | D6 |
| **R2: Readiness barrier** | Runtime and capability checks; verified readiness per member; shared capability facts (configured, probed, measured, unknown); coordinated initial release | D3, D7 |
| **R3: Liveness** | Explicit waiting states; deadlines; watchdog that tells inactivity, approval waits, dependency waits, provider failures and unknown outcomes apart; cycle detection where applicable; progress-based pause and recovery | D1 (general case) |
| **R4: User preference profile** | Persistent, user-controlled, versioned; hard restrictions, delegated scope, preferences and confirmed decisions kept apart; never authority | D9 |
| **R5: Plan-first collaboration** | Independent plans, exchange, negotiation, a shared plan, mechanical validation, explicit commitments, then asynchronous dependency-aware implementation; pause when planning does not converge; artifact handoff and replanning | D8 |
| **R6: Notification and escalation** | Local notifications; paired-device delivery; optional external channels where justified, carrying no authority; authenticated approvals through the existing path | D1 (unattended user) |

**Experiments**, kept apart so each change's effect can be seen; unavoidable differences are recorded, never silently changed:
- **E01-Retry**, after R0–R3: E01's objective, models, collaboration semantics and baseline as closely as possible. Does correct governance and readiness alone make collaboration meaningful?
- **E02**, after R4–R5: preference-guided decisions, shared planning, emergent responsibilities, commitments and asynchronous dependencies, compared with E01 and E01-Retry.
- **E03**: unattended collaboration across two physical devices, with remote escalation and interruption recovery.

Measures for each: human interventions; time to first delivered message; delivered messages; automatic turns used; duplicated files; stalls detected and their time to Needs you; builds and tests run per member; tokens per member.

**Still out:** an orchestrator model, a general task graph with assignment or priorities, and mid-turn delivery through Codex `turn/steer` (it breaks "nothing mid-turn" and parity with Claude Code).

## Progress

Recorded per milestone as it lands.

### R0: correctness and lifecycle closure (2026-11-25)

Boundary: make every state E01 hit visible and controllable, with no new authority and no new concept. Inspected first: `NativeDecisionReviewV1` already reached each task's status, and `cancel_task` / `stop_tasks_with_code` already existed; what was missing was a local decision path and the collaboration reading those facts.

| Item | Change | Evidence |
| --- | --- | --- |
| Native approval visibility | `RunningTurnV1` carries `awaitingDecision` and the turn's reviews; the member card reads "Waiting for your approval" and shows the existing review panel; a turn waiting on a request Pastey cannot show says so | `collaboration::tests::a_running_turn_waiting_for_a_native_decision_needs_the_user` |
| Local decisions | A Locally owned task's reviews keep Approve once / Deny (`ObservedTaskProgressV1::local_decisions`); `respond_local_native_decision` → `submit_local_native_decision`, refused for Bridge-owned tasks; peers still cannot decide a Local task; same digest, expiry and once-only rules (shared `decide_review`) | `native_agent::tests::a_local_task_takes_this_hosts_users_decision_and_only_that` (scripted Codex: decline reaches Codex once; stale digest, second answer, peer and Bridge-owned cases refused) |
| Needs you | Sidebar count and the view's Needs you include pending native decisions of running member turns (`needs_you_total_with`); each wait is recorded once in the Journal as a Pastey fact (not a second item) | same collaboration test |
| Member Stop | `stop_collaboration_member`: Stop's own cancellation of the running turn; the card shows "Busy · a turn is ending" until the native turn really ends; a collaboration turn that was stopped pauses its member (existing rule) | `ending_names_the_turns_the_collaboration_started_to_stop`; existing `stop_pauses_a_member_with_a_grant_and_its_turn_counts` |
| End stops its turns | Ending asks the turns the collaboration started (scheduled and automatic) to stop; a turn the user typed in a member's conversation is left alone | same |
| Start-failure diagnostics | A definite non-start (adapter `NotStarted` / refused start) pauses the member at once with the adapter's own reason (folder paths left out, bounded to what a linked device accepts) as an Open attention; no immediate retry (E01: three attempts in one second) | `a_turn_that_never_ran_pauses_its_member_with_the_reason_and_gives_up_after_three`, `paths_are_left_out_of_a_start_problem` |
| Worktree trust | Claude Code's trust recorded for a worktree's main checkout counts (`main_checkout` from the worktree's `.git` file) | `claude_code::conversation::tests::a_worktree_of_a_trusted_checkout_counts_as_trusted` |
| Model and effort | Stored per member (`MemberV1.options`), validated, passed on every turn the collaboration starts (Claude Code keeps no effort across `--resume`); a `user` entry records changes and stays on this device (like folders); member menu "Model and effort…" | `a_members_model_and_effort_reach_every_turn_the_collaboration_starts` |

Limits: the Journal note of a native wait is written by the scheduler (on a turn's end or its 30 s tick); the live count needs no tick. The trust rule for worktrees is inferred from one observation in E01 (Claude Code 2.1.318), not from documentation. No OS notification yet (R6). Not yet exercised against real Codex or Claude Code in the app.

### R1: execution profile (2026-11-25)

Boundary: carry user-authorised native settings to member turns; build no approval logic. Inspected first: a paired peer sends the same `CodexInvocationInputV1` as local starts, so a profile in invocation input would let a peer widen a Host's sandbox; Claude Code peers send a narrower input. The profile therefore never enters input.

| Item | Change | Evidence |
| --- | --- | --- |
| Profile type | `native_agent/execution_profile.rs`: Claude Code (permission mode, allow/deny rules, sandbox, allowed hosts, extra writable folders) and Codex (approval policy, reviewer, sandbox, network, writable roots); strict validation refuses `bypassPermissions`, `dangerFullAccess`, `untrusted`, multi-line rules, path-like hosts, root/home folders, `..` | `execution_profile::tests` |
| Presets | Ask (nothing passed) and Unattended engineering (Claude: `acceptEdits`, sandbox with auto-allow, registries only, `~/.cargo` `~/.npm` `~/.rustup` writable, push/PR/release/publish/sudo denied; Codex: `workspaceWrite` + `~/.cargo` `~/.npm`, network off, `on-request` decided by `auto_review`) | same |
| Core path | Held per task (`register_execution`), taken at that start, attached only to a Local conversation | `a_members_execution_profile_reaches_codex_turn_start_and_nothing_else`, `a_registered_profile_never_reaches_a_bridge_owned_task` |
| Adapters | Claude Code: `--permission-mode` and one merged `--settings` (only on a member turn); Codex: fields added to `turn/start` (`run_turn_with`) | `a_members_execution_profile_reaches_claude_code_as_native_settings` and the Codex test above |
| Hierarchy | User default (table `execution_defaults`, present only while authorised) → collaboration choice → member choice (`inherit`, `ask`, `profile`); consent required for every profile; resolved per turn, also for turns the user types in a member's conversation; `user` entries record changes on this device only | `execution_profiles_resolve_member_then_collaboration_then_the_users_default` |
| UI | Settings → Unattended collaboration (set up, change, revoke); Execution… on a collaboration and on a member; the card shows the effective profile, its source and "configured, not verified"; consent shows exactly what each Agent gets and that auto-review is a judgement | build + type check |

Live checks against the installed runtimes (observer probes, low effort, scratch folder):
- **Claude Code 2.1.318** (`claude-haiku-5-5`, `--permission-mode acceptEdits`, the preset's settings): `cargo --version` ran sandboxed without a prompt; `git push origin main` was denied by the deny rule (`permission_denials`). US$0.004.
- **Codex 0.170.0** (`codex app-server`, `turn/start` with `approvalPolicy: on-request`, `approvalsReviewer: auto_review`, `sandboxPolicy: workspaceWrite`, network off, effort low): fields accepted; `cargo --version` exit 0 with no request; `curl https://example.com` failed in the sandbox (exit 6) and Codex **did not escalate**, so `auto_review` was never asked. The thread's own default in an untrusted folder was `readOnly`, reviewer `user`.

Limits and open decision:
- **Codex cannot fetch dependencies under the preset.** Network off makes registry access fail rather than escalate; 0.170's only native switch is `networkAccess: true`, which is all outbound traffic (domain lists exist only as the experimental `features.network_proxy`, not settable per turn as measured). Whether "Unattended engineering" should turn Codex's network on is a security choice left to the user; the profile can be edited to do it.
- Claude Code's effort and `auto` mode are not observable from its output; the preset avoids `auto` and uses the sandbox as the boundary.
- Codex keeps turn overrides on the thread (see Layer 5).

### R2: readiness barrier (2026-11-25)

Boundary: no member turn of a new collaboration starts until every member is ready; readiness is checked without a model call; each member's abilities reach the others as facts with their basis. No scheduler, no inference.

| Item | Change | Evidence |
| --- | --- | --- |
| Native checks | `NativeCapabilityAdapterV1::readiness_checks` (default none): Codex `codex login status`; Claude Code `claude auth status` (`loggedIn`) and its folder-trust rule; each bounded to 10 s and run off the service lock (`NativeAgentServiceV1::readiness_probe`), after the capability's own availability | live: `installed_agents_answer_readiness_without_a_model_call` (ignored; ran on this Mac: Codex 0.36 s, Claude Code 0.30 s, all passing) |
| Member readiness | `collaboration/readiness.rs`: folder, installed, signed in, trusted (probed); model and effort, execution profile (configured); a check nobody could answer is not a pass; a member on the linked device is ready once it accepted | `a_new_collaboration_starts_no_turn_until_every_member_is_ready` |
| Facts | From the effective profile (configured: what it edits, runs, reaches, may not do, who decides escalations), fixed tool probes of this device (probed, labelled "outside any Agent's sandbox"), the member's last completed turn (measured) | `profile_facts_say_what_a_member_can_and_cannot_do` |
| Barrier | `CollaborationRecordV1.released` (collaborations from before R2 count as released); `plan_starts` starts nothing before release; messages sent meanwhile wait; `release_collaboration` checks again and refuses unless every member is ready; all members then start together | same test |
| Shared facts | Kept per member at release; every note lists them per member with their basis ("do not assume more") | same test (both prompts carry both members' facts) |
| UI | "Preparing" card: per-member checklist with basis, the facts others will be told, Check again, Start collaboration (enabled only when all are ready) | build + type check |

Limits: readiness is a snapshot at release (sign-in or quota can change later; M2's fast-failure pause still applies). Tool presence is probed in Pastey's environment, not inside an Agent's sandbox (Codex's `command/exec` could probe inside it; not used yet). Claude Code's effort is still not observable. A member added after release has no facts until a later check.

### R3: liveness (2026-11-25)

Boundary: make every way a collaboration can stop moving visible and bounded, with deterministic rules on declared structure and observed facts only. No scheduler beyond the existing turn boundary; nothing is delivered mid-turn; a deadline starts no turn.

| Item | Change | Evidence |
| --- | --- | --- |
| Explicit waits | A fenced `pastey-wait on=<member or user>` block (`for:` one line, `until:` 5 min to 24 h, default 2 h; one per reply), parsed with the same rules as messages; the note teaches it. A wait on a member needs nobody; on `user` it needs the user | `published::tests::a_wait_is_one_bounded_block_at_the_end` |
| Yield and resume | The waiter's turn has ended; its wait ends when the awaited member messages it (or the user does, for `user`), or when the waiter runs another turn (it re-declares if still waiting). Every prompt to the awaited member says who waits for it | `a_declared_wait_yields_the_turn_and_ends_when_answered_or_at_its_deadline` |
| Deadlines | At the deadline (scheduler tick) the wait becomes `expired` with an Open item for the user; the waiter stays idle and uses no turn | same |
| Cycles | A wait that closes a cycle of members waiting on each other (wait-for graph, depth-first over at most six members) is flagged at once as an Open item naming the cycle; Pastey breaks nothing | `members_waiting_on_each_other_are_flagged_at_once` |
| Watchdog | `MemberConditionV1` per member: working (with minutes since the harness last reported, `TaskActivityV1::last_at`), approval wait, turn ending, dependency wait, provider failure (failed before any action), outcome unknown, paused, idle. A running turn quiet for 30 min gets one Journal fact saying quiet is not failure | `the_watchdog_tells_a_provider_failure_and_an_unknown_outcome_apart`, frontend label tests |
| Progress pause | Automatic turns in a row that changed no file (scan, else harness report) and recorded no stage: at 4 the collaboration pauses with an Open item; the user's resume or message counts afresh. The hop limit and budgets stay as backstops | `automatic_turns_without_progress_pause_the_collaboration_for_its_user` |

Limits: waits stay on this device (a linked device does not receive them; the waiter's message conveys the need). "Progress" is files or a recorded stage; a turn that only reads and answers usefully counts as no progress, so a long purely analytical exchange pauses after 4 automatic turns. Cycle detection covers declared waits only, not implicit ones ("I'll wait for your reply" in prose).

### R4: user preference profile (2026-11-25)

Boundary: Pastey stores, versions and delivers the user's preferences; native Agents interpret them; nothing in them is authority.

| Item | Change | Evidence |
| --- | --- | --- |
| Profile | `collaboration/preferences.rs`: four sections kept apart (hard restrictions, delegated decision scope, ordinary preferences, decisions already made), each at most 1,200 characters and checked with the shared faithful-rendering rule; stored in `user_preferences`, a new version on every save | `preferences::tests::preferences_are_delivered_as_the_users_text_and_never_as_permission` |
| Collaboration context | A collaboration's own preferences (`preferences_note`), and the decisions the user confirmed in its Journal, join the profile in one section | `preferences_reach_each_member_once_per_version_and_grant_nothing` |
| Delivery | A member's turn carries the section in full when it has not had this text (digest kept per member at the turn's start); later turns do not repeat it; a new version or note reaches every member again; a turn the user types carries it too; it counts against the prompt's bound | same |
| Guidance, not authority | The section says it grants no permission; it tells Agents to make routine, reversible decisions within the delegated scope and record them with a reason in the summary's `decisions`, and to escalate only material uncertainty, substantial risk or decisions outside scope, with what is needed, why, the options and a recommendation. A preference never creates an execution profile; enforcement of a restriction belongs in the profile's deny rules (the UI says so) | same; Settings text |
| UI | Settings → Preferences for Agents (four fields, version shown); Preferences… on a collaboration | build + type check |

Limits: whether Agents follow the profile is their behaviour, measured in E02, not something Pastey checks. Restrictions written only as preferences are not enforced.

### R5: plan-first collaboration (2026-11-25)

Boundary: Agents plan, negotiate, decide responsibilities and commit; Pastey records, checks structure, enforces the phases and tells members what changed. No roles, no assignment, no ordering by Pastey; the representation is tasks only, not a project-management system. This amends the collaboration design's non-goal "not a task board", as decided: claims, dependencies, commitments and states, all Agent-authored.

| Item | Change | Evidence |
| --- | --- | --- |
| Plan blocks | One `pastey-plan` block: a proposal (`understanding:`, `task <id>: <what> \| owner= \| paths= \| after= \| deliver=`, `integration:`), an acceptance (`accept=rN`, committing to one's own tasks), or progress (`done: <task> artifact=<branch>@<commit>`, `blocked: <task> <why>`); at most 6 KiB, 30 tasks; paths only inside the owner's folder | `plan::tests` |
| Lifecycle | Start offers "Plan together first" (default on). Planning: members' turns run in their native read-only modes (Claude Code `plan`, Codex `read-only`; deny rules kept, never wider than the member's profile) and the note shows the latest proposal, Pastey's checks, every member's stance and planning turns left. Each proposal becomes revision rN and is sent to the other members; an acceptance moves the member's stance | `members_plan_read_only_converge_on_one_valid_plan_and_then_implement` |
| Mechanical validation | Unique task ids; owners are members; dependencies exist and form no cycle (Kahn); no two owners' path claims overlap (literal prefixes, conservative); an owner whose execution profile cannot edit is not given file claims | `plan::tests::validation_finds_overlaps_cycles_missing_owners_and_authorization` |
| Convergence | Adopted only when every member's stance is the latest revision and it has no problem; then every member is told its commitments and which tasks it may start now (messages from `pastey`, delivered under its grant or the user's approval) | first test |
| No convergence | 3 planning turns per member: when one runs out, the collaboration pauses with the state (latest revision, its problems, who accepts it); nothing is implemented. The user may adopt a revision without problems (a `user` entry), or resume for another round. If planning stalls with nobody to act (all accepted a revision with problems), every member is told the state, so planning continues until it converges or pauses | `a_plan_that_does_not_converge_pauses_and_conflicting_work_is_never_released`, `the_user_may_adopt_a_valid_revision_and_claimed_files_crossed_are_flagged` |
| Asynchronous implementation | Tasks with no dependency start at once; a `done` report tells the owner of each task whose dependencies are now all done; a `done` artifact `<branch>@<commit>` is checked read only (`git cat-file -e`) in the reporter's repository and recorded as found or not | first test |
| Claims | Files a turn changed that another member's task claims are flagged (not reverted) | third test |
| Replanning | A proposal during implementation returns the collaboration to planning with a fresh budget | covered by the lifecycle rules |
| UI | Start's "Plan together first"; a plan card (phase, latest or adopted plan, checks, task states, Adopt rN for a revision without problems); Journal lines for proposals, acceptances, adoption and progress | build + type check |

Limits: plan-first is refused for a collaboration spanning another device, and plan entries stay on this device. Path overlap is literal-prefix based, so `src/**` and `src/a/**` overlap while sibling globs with different prefixes never do. "Done" is the Agent's statement; Pastey checks only that a named commit exists. Whether Agents plan well is their behaviour, measured in E02.

### R6: notification and escalation (2026-11-25, first part)

Boundary: the user learns, without watching Pastey, that something needs them; notification is separate from authority. No new dependency and no Pastey cloud.

| Item | Change | Evidence |
| --- | --- | --- |
| What is told | Each new item that needs the user, once: a member's question (message to `user`, or a wait on `user`), a message to approve, a problem (attention), and each native decision a running member turn waits for. Items that already needed the user when Pastey started are recorded, not re-announced | `each_new_item_that_needs_the_user_is_notified_once` |
| Local channel | An OS notification on this device (macOS `osascript`, Linux `notify-send`; the text is passed as arguments, never as script); on by default | live: a test notification on this Mac (exit 0, quotes and `$(…)` shown as text) |
| External channels | An optional program the user names by full path, run with one JSON argument (bounded, off every lock): the user's own tools can email, text or push. It receives only `{type, count, kinds}` unless the user allows details (collaboration, member, subject); internal ids never leave | `notify::tests::a_program_gets_only_that_something_needs_you_unless_details_are_allowed` |
| Authority | Nothing in a notification can approve; the payload says so. Answers and approvals stay in Pastey: the local decision path (R0) and the paired device's existing digest-bound remote decision path | by construction |
| UI | Settings → When a collaboration needs you (local on/off, program, details, Send a test) | build + type check |

Escalation content (what is needed, why it matters, the options, a recommendation) is asked of Agents by the preference section (R4); Pastey forwards the subject.

Not yet: notifying a paired device over its Bridge (it needs a new Room Control event; planned before E03, which needs two devices anyway), and Windows local notifications. Until then the program channel covers remote delivery with the user's own tools.

### Check in the running app (2026-11-26)

The debug bundle at `75a060d` was run with the user's real app data and driven through its own screens (a throwaway collaboration on two scratch folders, deleted afterwards; the default profile dialog opened and cancelled, nothing saved).
- Settings → Agents shows Unattended collaboration, Preferences for Agents and When a collaboration needs you. The default-profile dialog lists exactly what each Agent gets, warns that auto-review is a judgement and not a boundary, and keeps Authorise disabled until consent is ticked.
- A new collaboration opens in Preparing: both members ready, with sign-in checked live through `codex login status` and `claude auth status`, each check labelled checked now, configured or not known; "Plan together first" on by default; the facts each member would be told are listed. Member cards show model and effort and "Execution: Ask · nothing chosen · configured, not verified"; the menu offers Preferences… and Execution….
- Two defects found and fixed: the profile JSON and preference fields rendered one line tall (wrong style class), and the readiness line read "Model and effort: Model and effort: …".

Not yet run: E01-Retry (deferred: no Codex quota on 2026-11-26), E02, E03; paired-device notification and Windows local notifications (R6, second part).

## Sources

- Claude Code: [permissions](https://code.claude.com/docs/en/permissions), [permission modes](https://code.claude.com/docs/en/permission-modes) (partly read), [headless](https://code.claude.com/docs/en/headless), [hooks](https://code.claude.com/docs/en/hooks) (partly read), [sandboxing](https://code.claude.com/docs/en/sandboxing) (partly read), [settings](https://code.claude.com/docs/en/settings), [agent teams](https://code.claude.com/docs/en/agent-teams), [agents](https://code.claude.com/docs/en/agents), [dynamic workflows](https://code.claude.com/docs/en/workflows), [subagents](https://code.claude.com/docs/en/sub-agents) (partly read), [cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging), [agent view](https://code.claude.com/docs/en/agent-view) (partly read), [common workflows](https://code.claude.com/docs/en/common-workflows).
- Codex (now served from learn.chatgpt.com): [app server](https://developers.openai.com/codex/app-server), [approvals and security](https://developers.openai.com/codex/agent-approvals-security), [configuration reference](https://developers.openai.com/codex/config-reference), [advanced configuration](https://developers.openai.com/codex/config-advanced), [non-interactive mode](https://developers.openai.com/codex/noninteractive). The rules, hooks and subagents pages were not read.
- Installed: `codex app-server generate-json-schema` (0.170.0): `TurnStartParams`, `ThreadStartParams`, `AskForApproval`, `ApprovalsReviewer`, `SandboxPolicy`, `ThreadActiveFlag`, `CommandExecParams`, `ClientRequest`. `claude --help` (2.1.318).
- Coordination: Gray & Cheriton, *Leases* (SOSP 1989); Chandy, Misra & Haas, *Distributed deadlock detection* (TOCS 1983); Kahn, *Topological sorting of large networks* (CACM 1962); Garcia-Molina & Salem, *Sagas* (SIGMOD 1987).
