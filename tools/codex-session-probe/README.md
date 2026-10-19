# codex-session-probe: continuity with desktop Codex sessions (D2 research)

Research only. Nothing here is part of the Pastey app. The question: once Pastey runs Codex itself (D1), can it continue the user's existing ChatGPT desktop Codex sessions? Three separate capabilities, with separate evidence:

1. **History:** list or read the desktop's threads.
2. **Resume:** reopen a historical thread and run a new turn in it.
3. **Attach:** join the turn the desktop is running now, and control it (see its events, answer its approvals, interrupt it).

1 and 2 are not 3. Only official entry points were used: the `codex` executable bundled with the app and its documented app-server protocol. No database writes, private tokens, internal IPC (`~/.codex/ipc`) or UI automation.

## Tools

- `probe.py` (read only): starts its own app-server and runs `thread/list` (state DB only), a metadata `thread/read` of the newest desktop thread, and `thread/loaded/list`. It prints counts and statuses only, never titles or content.
- `attach_poc.py`: starts its own app-server on a private Unix socket in a new temporary folder, connects two clients A and B, and shows what a second client of **the same server** can do with A's thread. `--turn` runs one small model turn that must ask for a command approval, and records who receives the request, who may answer it and what the other client sees. It never connects to the desktop's app-server or to the shared daemon.

```bash
python3 -I tools/codex-session-probe/probe.py /Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex
python3 -I tools/codex-session-probe/attach_poc.py /Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex --turn
```

## Evidence, 2026-10-16, MacBook (macOS 26.5.2), ChatGPT.app with bundled `codex-cli 0.162.0-alpha.17.2`

Evidence levels: **real** = run on this Mac against the installed app; **docs** = the bundled CLI's own `--help` or its generated protocol schema (`codex app-server generate-json-schema`); **inferred** = reasoning from those, not observed.

### How the desktop runs Codex (real)

- The desktop starts its own `codex app-server` as a child process. Its only Unix sockets are connected socketpairs: there is no listening socket, port, or control socket.
- `codex app-server daemon version` fails because `~/.codex/app-server-control/app-server-control.sock` does not exist. No shared daemon is running.
- The desktop's app-server is therefore reachable only by the desktop itself. No supported way exists to connect a second client to it.

### 1. History: supported (real)

A separate app-server, started from the same bundled executable, sees the desktop's history through the shared `~/.codex` state:
- `thread/list` returned 50 threads: 43 with originator "Codex Desktop", 7 "Pastey" (D1 runs).
- `thread/read` with `includeTurns` read the newest desktop thread: 10 turns (7 completed, 3 failed) and 3,085 items.

**Limit:** every thread showed `notLoaded`, including any the desktop may have open. Thread status is per app-server, so a separate process cannot tell whether the desktop is running a thread right now.

### 2. Resume: works technically, unsafe as a product feature (real + inferred)

- Pastey already resumes its *own* threads after a restart with `thread/resume` (Layer 5), and that works with the desktop backend.
- The same call accepts a desktop thread id: it loads the rollout from disk in Pastey's app-server and can start a new turn there.
- `attach_poc.py` shows a thread has no rollout until its first turn: resume before then fails with `no rollout found`.

Why not ship it:
- **Two writers (inferred).** If the desktop has the thread open, two independent app-servers append to the same rollout, each with its own in-memory history. Neither sees the other's turns until reload, and the protocol has no cross-process lock or ownership signal. Point 1's limit means Pastey cannot even detect that case.
- **Ownership.** A resumed desktop thread would continue under the trust and approval policy of whoever resumed it. A remote requester could extend a conversation the local user started in another app. That changes Pastey's session-binding rule (one owner per conversation), so it would need its own product decision and Review.

### 3. Attach: not supported for the desktop's sessions (real + docs)

- **Same server only (docs).** The protocol supports it within one app-server. The `ThreadResumeParams` description says: if `thread_id` identifies a running thread, "app-server rejoins that thread". `thread/unsubscribe` exists, and `thread/started` is broadcast to every connection.
- **Same server, real.** `attach_poc.py` confirmed that client B of one server sees A's loaded thread in `thread/loaded/list` and receives its `thread/started`. The turn-level results, below, show what B can do during A's running turn.
- **Not across processes (real).** The desktop's server accepts no second client, as shown above. Pastey cannot attach to, observe or answer the desktop's running turn, and it does not try to.

### Official entry points (docs)

| Entry point | What it is | Can Pastey use it for 3? |
|---|---|---|
| App Server over stdio | What D1 uses: Pastey's own private app-server. | No: it is a separate server from the desktop's. |
| `app-server --listen unix://…` or `ws://…` (with `--ws-auth` capability-token or signed-bearer-token) | One app-server serving several clients. | Only if the desktop were a client of that server. It is not. |
| `codex app-server daemon` (`start`, `bootstrap` "for SSH-driven use", `enable-remote-control`) and `proxy --sock` | A shared local daemon with a control socket. `codex agents` browses "all agent sessions on the shared local app-server daemon", and the TUI uses the shared background server when it runs. | **Upgrade boundary**, below. |
| `remoteControl/*` and `codex remote-control` | Connects an app-server to ChatGPT's remote-control service, for ChatGPT clients. | No: it is OpenAI's own client channel, not an API for third parties. |
| `codex mcp` / MCP | Codex as an MCP client of external tools. | No: it adds tools to Codex, but grants no control of a session. |

## The upgrade boundary for 3

Attach becomes possible, through supported means only, when the desktop and Pastey are clients of **one** app-server. That means the shared daemon, if the desktop uses it, for example for its SSH remote connections. That was not observed here: no daemon ran, and the local desktop uses a private child (**inferred** from the CLI's help). If that changes, Pastey would connect through `codex app-server proxy --sock` or the daemon's socket, `thread/resume` the running thread, and receive its events.

Even then, these questions must be settled first:

- **Write ownership.** Every client can `turn/start` and `turn/interrupt` on a shared thread. Pastey must not start turns on a thread whose owner is the desktop user without an explicit local handover. It should default to observe-only.
- **Approval ownership.** The turn-level run below records whether the server sends an approval to one client or to all, and whether the first answer wins. Pastey's rule stays as it is: only the device that started a task may answer its approvals. On an attached desktop turn that device is the desktop's user, so Pastey must not answer, and should show the approval as "waiting in the Codex app".
- **Disconnect ownership.** For `command/exec`, the protocol says that when the originating connection closes, the server ends the process (connection-scoped). Whether a turn survives its starting client's disconnect, and who may then interrupt it, must be measured on the daemon before relying on it.
- **Idempotency.** Turns have no client-supplied idempotency key. A resend after a lost answer can start a second turn. Pastey's at-most-once rule (never re-send a turn) must carry over unchanged.
- **Trust.** A daemon reachable over a socket or WebSocket is a new local authority surface. A Bridge peer must never reach it directly: only the Host's own Pastey, under the existing Device Trust and Bridge Authority, may act there.

## Turn-level results (`attach_poc.py --turn`), 2026-10-16 05:59 (real, one isolated app-server)

| Observation | Result |
|---|---|
| B `thread/resume` while A's turn runs | Same thread, with the turn `inProgress`: B rejoined the running turn. |
| Turn events | B received every item, delta, status, token-usage and `turn/completed` notification, the same counts as A. |
| Approval | The same `item/commandExecution/requestApproval` went to both A and B. |
| Who answered | B answered. The command ran, the file was written, and the turn completed. |
| After the answer | Both received `serverRequest/resolved`. |
| Not measured | Whether a second answer from A, after B's, is rejected or ignored. Whether the turn survives A disconnecting. |

So within one server, an approval goes to **every** subscribed client, and any of them can answer it. The server gives no notion of an approval's owner. A client that must enforce ownership, as Pastey does, has to enforce it itself, and cannot stop another client from answering first. This is the main reason attach would need an explicit ownership design, as listed above.

## Conclusion

- **History (1):** supported and real. Pastey can list and read the desktop's threads through its own app-server, but cannot see which of them the desktop is running.
- **Resume (2):** technically possible for desktop threads, and real for Pastey's own threads. It is not safe as a feature while the desktop may hold the same thread, because nothing detects or prevents two writers.
- **Attach (3):** not available for the desktop's sessions on this version. The desktop's app-server is private to it. The protocol supports attach only among clients of one shared app-server (shown here with a minimal two-client run). It becomes reachable only if the desktop runs its sessions on the shared daemon; that would also need write and approval ownership, disconnect and idempotency rules, and a trust boundary decided first.

Pastey does not claim 1 or 2 as 3, and ships none of them as a product feature.

## Update 2026-10-19: Codex 0.163 thread writer locks

Re-checked on the same Mac with ChatGPT 26.1013 (bundled `codex-cli 0.163.0-alpha.10.2`) and the CLI `0.163.0`. **Real** unless marked.

- **The desktop's app-server is still private.** It is a child of the app with no listening socket or port, and no shared-daemon control socket exists. Attach (3) stays unavailable.
- **New: a per-thread writer lock.** An app-server that loads a thread keeps `~/.codex/thread-writer-locks/<thread>.lock` open with an exclusive BSD `flock`, and removes the file when it exits (`thread/unsubscribe` alone did not release it within a second). The desktop held exactly the locks of the threads it had loaded.
- **It is enforced across processes and versions.** `writer_lock_probe.py`: a second app-server's `thread/resume` of a held thread is refused with "thread … already has an active writer", both CLI ↔ CLI and CLI or bundled Codex against a thread the desktop holds. After the holder exits, resume succeeds. Another server's `thread/list` still says `notLoaded`, so protocol status alone is no cross-process signal; the lock is.
- **It can be observed without taking it.** On macOS a POSIX `F_GETLK` reports the flock (pid -1) and acquires nothing.
- **Continuation keeps the thread.** `continuity_probe.py`: a thread started in one process, then continued from another, kept its id, its earlier turn, and its context (the model recalled a code word from the first process's turn). `clientUserMessageId` is persisted in the history.
- **`thread/resume` bumps `updatedAt`.** A precondition must therefore use the newest turn id, not the time.

This changes conclusion 2 above: resuming a desktop thread is now safe whenever the resuming client holds the writer lock, because Codex itself refuses the other writer. Pastey ships it as `agent.coding.codex_thread` (Layer 5, "Codex threads"); the adapter's live tests (`installed_codex_continues_a_thread_with_its_context_and_releases_it`, `installed_codex_refuses_a_thread_another_client_holds`) passed on this Mac. **Not verified:** the same enforcement on Windows and Linux (inferred from the shared implementation), and what the Codex app shows when the user opens a thread while Pastey's turn holds it (expected: Codex's own "active writer" refusal there).

```bash
python3 -I tools/codex-session-probe/writer_lock_probe.py "$(command -v codex)" --cross-process
python3 -I tools/codex-session-probe/continuity_probe.py "$(command -v codex)"
```
