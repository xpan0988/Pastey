# native-session-handoff: observing and taking over a running turn (S3 research)

Research only. Nothing here is integrated into the Pastey app.

The question: when Claude Code or Codex is **already executing** a turn (in a desktop app or a terminal), can a second client observe it, send it a follow-up, or take over control and approvals safely, through official interfaces? Private IPC, database writes and GUI automation are out of scope. See `tools/desktop-ui-probe` for why UI automation was not pursued.

Evidence levels:
- **real:** run on a MacBook (macOS 26.5.2) with Claude Code 2.1.292 and ChatGPT.app's bundled Codex 0.162.0-alpha.17.2;
- **docs:** the CLI's own help, or Codex's generated protocol schema;
- **inferred:** reasoning from those.

## Tool

`observe_claude.py <workspace> <session-id> [--seconds N] [--show-text]` is a read-only second client of one Claude Code session.
- It polls `claude agents --json` and tails the session transcript.
- It prints status changes and the kind of each new entry (prompt, text, tool use, tool result, end of turn), without content unless asked.
- It sends nothing and cannot answer an approval.

For Codex, `tools/codex-session-probe/attach_poc.py` is the two-client app-server PoC from D2.

## Findings

### 1. Can a second client observe a running turn?

| | Claude Code | Codex |
|---|---|---|
| Read-only observation | **Yes, real.** The live registry (`claude agents --json`) gives held or not held and `busy`/`idle`, for interactive, headless and background sessions alike, the desktop app's Code sessions included. The transcript is appended while the turn runs, and the observer saw each entry within about 0.4 s of its write time. | **Within one app-server only, real (D2).** A second client of the same server receives every event and approval. A separate server reads persisted history but reports any thread it has not loaded as `notLoaded`. How fresh that history is during a desktop turn is **untested**: the Codex quota was exhausted. |
| Granularity | Per completed step: prompt, tool call, tool result, message. The 371-character reply appeared once, when the turn ended, not streamed. | Same server: streamed deltas. Separate server: persisted items. |
| Desktop's own turn | Yes: the desktop app's Code sessions use the same registry and transcripts. | No: the desktop's app-server is a private child with no listening socket. |

The reliable read-only mode is therefore:
- **Claude Code:** registry plus transcript tail. A turn's progress is visible step by step.
- **Codex:** only for turns run by a server Pastey is a client of.

### 2. Can a follow-up be submitted to a running turn through an official interface?

- **Claude Code: no programmatic way (docs + real).**
  - Interactive sessions queue messages typed during a turn (transcript `queue-operation` records), but nothing lets another process enqueue one.
  - `claude --bg --resume` of a held session starts a **copy** (real); a headless `-p --resume` writes concurrently and branches the history (real).
  - `claude attach <id>` hands a **background** session to a terminal UI. That is a human handoff: driving it from a program would be terminal automation.
- **Codex: yes, within the same app-server (docs).** `turn/steer` takes `threadId`, `input`, a required `expectedTurnId` (it fails when that is not the active turn) and an optional `clientUserMessageId`. It is reachable only by clients of the server that runs the turn, so not the desktop's private server. Untested: no Codex quota.

### 3. Who owns execution and approvals, and how to prevent double control

- **Claude Code:**
  - The process holding the session owns it, and the registry names it.
  - Approvals belong to that process's own UI. A background session's prompts wait for `claude attach`; Pastey never answers them.
  - Claude Code prevents two writers only on some paths:
    - `--bg --resume` copies instead of writing a held session (real, for background and foreground holders);
    - `-p --resume` refuses a session held **in the background** (real) but not one held in the foreground (real: concurrent write).
  - Pastey's continuation (S2) therefore uses only `--bg --resume`, checks the registry first, and releases the session with `claude stop` when done.
- **Codex:**
  - Within one server, every subscribed client receives each approval, and any of them can answer it (real, D2).
  - Every client can `turn/start`, `turn/interrupt` and `turn/steer`.
  - There is no notion of an owner, so double control is possible and must be prevented by the clients themselves.
  - Across servers there is no lease at all.
- **Transfer:**
  - Claude Code transfers control of a background session by `claude attach` (a human), and releases it by `claude stop`.
  - Codex has no transfer: a server's clients share control.

### 4. Disconnects, restarts and reconnection

- **Claude Code background sessions** run in Claude Code's background service, not in Pastey (real: "Starting background service…"). They outlive a Pastey restart.
  - After a restart Pastey can reconcile from the registry (still held? `busy`/`idle`?) and the transcript (did the turn end, and how?).
  - The S2 adapter currently relies on Core's existing rule instead: an unobserved invocation becomes interrupted or needs reconciliation after a restart. Registry-based reconciliation is the natural next step and is **not implemented**.
- **Codex in Pastey's own app-server** ends with Pastey: the turn stops with the process.
  - Codex's shared daemon (`codex app-server daemon`) would survive a client disconnect.
  - Whether a turn survives its starting client's disconnect there is **untested**. For `command/exec`, the schema says the process ends with its connection.
- **Reconnection:**
  - Codex `thread/resume` of a running thread rejoins it within one server (docs, real in D2).
  - Claude Code `claude attach` reopens a background session; `--bg --resume` with no options continues ("woke session …", real).

### 5. Provider differences

| | Claude Code | Codex |
|---|---|---|
| Live "is it running" across processes | Yes: `claude agents --json` | No (only "no Codex process at all") |
| Exclusive continuation | Yes: `--bg --resume`, copy-on-held | No lease; resume from a second server can double-write |
| Steering a running turn | No | `turn/steer`, same server only |
| Observing a running turn | Yes: transcript, step granularity | Same server only |
| Approval ownership | The holding process's UI | Any subscribed client |
| Handoff | `claude attach` (human), `claude stop` | None |

## Technical boundary and what an upgrade needs

Taking over a turn that is **already running** is not supported safely by either provider's official interfaces today:
- Claude Code offers observation and a human handoff, but no programmatic steering.
- Codex offers steering and rejoining only among clients of one server, without ownership, and its desktop app does not share its server.

Pastey therefore implements none of it in production.

| Path | Needed before production |
|---|---|
| Claude Code: observe a running session | Low risk: registry plus transcript tail, read only. Could be added to discovery as "live progress". It must keep facts (entries written) apart from claims (the agent's text). |
| Claude Code: follow-up while running | Only possible today as "after the turn ends": wait for `idle`, then continue (S2). Real steering needs a Claude Code interface to enqueue a message into a held session. |
| Codex: observe, steer, approve a desktop turn | The desktop would have to run its threads on the shared daemon, so Pastey could join as a client. Then still needed: an ownership rule (who may steer, interrupt, approve), observe-only by default, `expectedTurnId` / `clientUserMessageId` for idempotency, disconnect semantics measured on the daemon, and a trust boundary for the daemon socket. Bridge peers never reach it directly. |
