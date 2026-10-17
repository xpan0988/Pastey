# Remote continuation of a Claude Code session: design and blockers (Phase 2)

Status: **not enabled.** Local continuation (S2, `agent.coding.claude_code`) stays local-only. The adapter advertises no peer protocol, peers are not told it exists, and Core refuses any Bridge invocation of a capability without peer protocols (tested: `no_bridge_peer_can_invoke_it_or_learn_of_it`).

Remote continuation is allowed only once every boundary below is shown to be handled. Three are not, so it stays off.

## The intended path

It reuses the existing lifecycle, and no new one:
1. On the Mac, the user reads Windows's sessions (remote session reading) and chooses one.
2. The Mac sends the ordinary Native Agent invocation (`native_agent.invoke`, invoke-v2) for capability `agent.coding.claude_code`. The input names the session, the scope and the prompt.
3. Windows holds it for the executor Review, exactly like any remote invocation. Only its user's Accept starts it.
4. After that come task identity and replay, status, Stop, Burn and reconciliation, all through Core, unchanged.

## Boundaries

| Boundary | Today | Needed before enabling |
|---|---|---|
| **Duplicate submission, concurrent writes** | Handled. Core task identity and replay make a task start at most once. The adapter's exclusivity key allows one invocation per session. The adapter never resends. Claude Code's `--bg --resume` starts a copy instead of writing a held session (measured). | Nothing. |
| **Held by another process at start** | Handled. `claude agents --json` is checked again immediately before launch, and copy-on-held covers the race after it (measured). | Nothing. |
| **The right session** | **Gap.** The invocation names a session id inside a permitted folder. Nothing checks that it is still the session the user inspected: someone may have continued it in between. | A precondition in the input: the transcript's last main-chain entry `uuid` and `lastActivityAtMs` the user saw. The adapter refuses (definite non-start) when either changed. Continuation also needs its own Host-local permission; a read permission must never imply it. |
| **A copy Claude Code started** | **Partly.** The adapter stops the copy and reports the outcome as unknown. Core records a summary only for completed and not-started outcomes, so the requester sees "outcome unknown" without "Claude Code started a copy, which Pastey stopped". | Record the adapter's summary for unknown outcomes too. That is a display fact, not a rule change; it must still never make the task completed. |
| **Pastey restarts while a background session runs** | **Gap.** The background session runs in Claude Code's own service and outlives Pastey. On restart, Core marks a Bridge task as needing reconciliation, and an unbound local task as failed. It never finds the session again, and never runs `claude stop`, so the session can stay held. Safety holds: later continuations are refused while it is held, and nothing writes twice. But the outcome and the release are lost. | Keep `(task id, session id, prompt marker, launch time)` durably with the envelope. On restart, reconcile through the adapter, from the registry (still held? `busy`/`idle`?) and the transcript (did the turn after the prompt end, and how?). Report the true outcome through the existing reconciliation fact, and release the session. This needs an adapter reconciliation hook in Core recovery, which Core does not have today. |
| **Pending native approval** | **Partly.** A background session waits for `claude attach` on the Host. Pastey shows the task as waiting for a native decision and never answers it. From another device, the task stalls until someone at the Host attaches, or it is stopped. | Either accept that a remote continuation may stall and say so on the requester's card (the Host must be attended), or relay approvals through Claude Code's permission prompt tool (an MCP `--permission-prompt-tool`). Relaying would need its own design, the same requester-only rule as Codex's decisions, and real-device evidence. |

## Order of work, when this is taken up

1. Session precondition (`uuid` + `lastActivityAtMs`) and refusal. Small, local, testable.
2. Unknown-outcome summaries recorded by Core.
3. Durable background-session record and restart reconciliation through an adapter hook. Prove it with a fixture `claude` that keeps running across a service restart.
4. A Host-local continuation permission, separate from reading. Then advertise peer protocols for the adapter, and only then remove the refusal.
5. Mac ↔ Windows physical acceptance: Claude Code's `--bg` on Windows has not been observed at all.
