# Layer 5 — Agent execution

## Role in Pastey

Layer 5 turns task decisions into execution on selected Host capabilities and handles completion, continuation, and consequences. Its implemented paths are native Host capabilities behind one capability-neutral lifecycle (Codex is the shipped adapter) and a separate Generic Managed Worker path. For managed Plans it owns semantic composition, optional proposal interpretation, immutable object-flow revisions, one complete requester approval, Host admission, exact attempt/step authority, managed execution, authoritative completion, and dependency continuation. It does not own transport routes, raw Host resources, or Developer Terminal authority.

## Agent / Host / capability model

A Host is a device and execution locality represented by Pastey. A capability is an ability exposed by that Host. An Agent is an intelligence or execution participant that can make or propose decisions over capabilities and resources. The general-capability direction lets an Agent reason over heterogeneous capabilities across Hosts without constraining it to one tool, provider, model, capability family, or locality. Current capability observation does not grant selection or execution authority, and Pastey does not currently integrate arbitrary Agents or capabilities. See [architecture](../architecture.md) for the system model.

## Native mature Agents

A mature Agent or other mature runtime is a Host capability, not a
Pastey-managed Worker. The native capability owns HOW; Pastey owns WHERE,
authority, executor admission, task ownership, cross-Host movement,
consequences, completion, cancellation, and recovery. The capability's native
provider/auth, model selection, input interpretation, workspace behavior,
tools, sandbox, reasoning, subagents, safety, and persistent conversation
remain Host-private and opaque to Pastey.

The default local path is intentionally direct:

```text
Pastey → selected Host → native Agent session → original workspace → native result
```

Pastey neither scans the workspace merely to observe edits nor creates a
ManagedObject, Scratch lease, provider broker, managed Worker run, or effect
translation. A Host-private native session is keyed by Host + Agent + canonical
workspace. Related tasks resume that session; an unrelated workspace receives a
different session. The native session identifier is never a Plan semantic.

## Native capability adapters

The native lifecycle in `NativeAgentServiceV1`—task identity, ownership,
admission, status, cancellation, reconciliation, restart recovery, Bridge
authority, and the durable envelope—is capability-neutral. Each Host
capability plugs in as a `NativeCapabilityAdapterV1` registered under its
capability ID, and Core dispatches every invocation by that ID. The adapter
owns HOW: what its input means, how it validates and starts, how it stops,
and what its terminal output contains. It reports only an identity digest, an
optional opaque exclusivity key, a renderer-safe label, and (for
workspace-scoped capabilities) its Host workspace. An adapter receives no peer,
session, Bridge, or Review context and decides no authority; it can only
refuse an input it cannot run or report a native failure.

Invocation input and terminal output are `OpaqueCapabilityPayloadV1`: JSON
owned by the capability, bounded to 16 KiB, which Core carries, persists,
compares, and digests but never interprets. Room Control's field-name safety
check skips exactly that subtree of `native_agent.invoke` (invoke-v2),
`native_agent.status`, and `native_agent.reconciliation`; every other field
is still checked. The renderer treats terminal output only as untrusted data.
An adapter that reports user-facing text uses the shared
`pastey-native-text-output-v1` shape (`text`, `truncated`), cut at a character
boundary to fit the bound; the task card renders it as escaped plain text,
separately from the lifecycle state, for any capability and any Host.

Codex is the only adapter registered in the shipped product. A test-only
`fake.longjob.v1` adapter proves the same lifecycle (identity, exclusivity,
cancellation, restart, Burn, admission) without Codex. The generic
`pastey-native-agent-invoke-v2` invocation and its
`start_remote_native_capability_task` command exist, but the renderer does not
call them yet; product remote Codex invocation still uses the original
`native_agent.invoke` v1 payload, which Core maps onto the Codex adapter's input.

## Native Codex today

The first concrete implementation is Codex via its native app-server session
protocol. Pastey inherits Codex's normal configuration and authentication and
uses `initialize`, `thread/start`, `turn/start`, bounded lifecycle observation,
`turn/interrupt` when available, and shutdown. It does not set a Pastey model
provider, inject credentials, select a model, impose `externalSandbox`, or use
an ephemeral session. Completion is accepted only from the exact requested
thread and turn with native `status: completed` and no error. A failed,
interrupted, cancelled, disconnected, malformed, or otherwise ambiguous native
turn is non-DONE.

The task's output is Codex's final user-facing reply: the completed
`agentMessage` items (`item/completed`) of the exact thread and turn, preferring
the last one Codex marks `final_answer`, otherwise the last one not marked
`commentary`. Streaming deltas, reasoning, plans, tool calls and command output
are never captured. App-server lines are read with a 1 MiB bound; a longer or
malformed line is skipped without ending observation, and a completion whose
reply could not be read, was empty, or was missing says so in its summary
instead of producing output. Output never changes whether the turn succeeded,
and a cancelled task never records it.

Codex may block a turn on a request only its own client answers: command,
file-change and permission approvals, user-input questions and MCP
elicitations. Pastey never answers them. The running task carries
`native_agent_awaiting_native_decision` (also on the requesting Host) until
Codex reports the request resolved; the workspace stays occupied and Stop
interrupts the turn, which clears the request natively. Other client requests
Pastey never advertised receive the protocol's method-not-found error.
Forwarding approval decisions between Hosts is not implemented.

Each Pastey-managed Codex conversation belongs to exactly one owner — this
Host's user, or one authenticated peer Host over one Bridge — and one
canonical workspace. When Codex confirms a new thread, Core saves its
continuation in the Host-private `native_agent_conversations` table (never
sent over a Bridge); the conversation's lifecycle is separate from any task
envelope. A task that continues (`resume: true`, every product path) reuses
that owner's live session, or after a restart or eviction reopens the saved
thread with `thread/resume` and requires Codex to return exactly that thread.
If it cannot be reopened (deleted, unreadable, unsupported, or another thread)
the task fails as a definite non-start with
`native_agent_conversation_unavailable`; Pastey never substitutes a new
thread. Only an explicit new conversation (`resume: false`) begins one and
replaces the saved record once Codex confirms it. Status carries only
`conversation: new | continued`, set once confirmed. Another owner, another
workspace, or a burned Bridge never reopens a conversation: Burn deletes the
Bridge's records and live sessions, the burned-Bridge tombstone refuses late
writes and reads, and records are capped (256). At most four idle live
sessions are kept; an evicted one is reopened when its owner continues.
Workspace exclusivity still serializes follow-ups, an uncertain turn at
restart still blocks them until reconciled or explicitly cancelled, and no
turn is ever re-sent.

A new session opens on the task's observer thread, so no process launch,
`initialize` or `thread/start` wait holds the native service lock; a stop
before the turn starts closes the opening session without sending the turn,
and a session that cannot open is a definite non-start
(`native_agent_start_failed`). Availability is a cached read-only observation
(two bounded probes, 30-second reuse per executable) and is observed afresh
before each new session. On Windows, `codex` resolves through absolute PATH
directories, preferring `codex.exe` over the npm `codex.cmd` shim within each;
the app-server is started in a non-verbatim working directory and shut down by
closing its stdin, which also ends it behind the shim.

Pastey uses either backend, and a separate CLI install is not required when the
ChatGPT desktop app with Codex is installed:

- **CLI (`cli`):** `codex` on PATH, as above.
- **Desktop (`desktop`):** the Codex the desktop app bundles, found only through
  the app's `codex-cli/codex-package.json` layout manifest.
  - The manifest must have `layoutVersion` 1 and, if it names one, variant
    `codex`.
  - Its `entrypoint` must be relative and stay inside that folder after
    resolving links.
  - macOS looks in `/Applications/*.app` and `~/Applications/*.app` under
    `Contents/Resources/codex-cli`.
  - Windows looks in the install location of OpenAI, ChatGPT or Codex app
    packages and in `%LOCALAPPDATA%\Programs`, up to five folders deep.

Pastey runs that executable exactly as it runs the CLI: the same probes, the
same `app-server` over stdio, and the user's own `~/.codex` configuration and
sign-in, which both backends share. It never reads the app's credentials,
changes its files, or talks to the app's own running app-server. Each new
session re-resolves the path, so an app update that moves it is followed.

Pastey tries the CLI first, then each desktop app, and uses the first one whose
probes pass. If none passes, it reports the first real failure (for example a
desktop Codex without `app-server` is `incompatible`), and reports
`native_agent_not_installed` only when neither was found. An explicitly
configured executable is used alone. The capability names the backend in use;
the backend is local, and peers see only availability.

The desktop backend is a separate process from the desktop app. It shares
history, configuration and sign-in with the app, but not the app's running
turns or approvals. Pastey's sessions appear in the app's history (as
originator "Pastey"), but the app cannot answer their approvals; Pastey's own
rules above apply.

### Native completion and consequence recovery

Pastey persists only its outer task and movement facts: immutable correlation, Host-private Bridge binding, execution status, exact result snapshot/digest, and requester apply state. Agent execution, result capture, encrypted Return, conflict retention, result Apply, and movement completion are separate facts. A completed Agent task stays `Completed` if capture, Return, retention, Apply, or explicit recovery abandonment later fails. Recovery, Return retry, cancellation, and Burn never authorize a second Agent turn. Restart does not recreate a native process, session, or turn. Unproved Bridge-bound execution becomes interrupted/reconciliation-required, while a task still awaiting executor Review never started and becomes a definite non-start; an unbound local task that cannot be reconciled becomes terminal `failed` with `native_agent_interrupted_on_restart` and its unreachable envelope is removed.

A durably completed received task can seal its exact result from its retained app-owned workspace if capture was interrupted, then retry only Return. Failed or unsafe capture is a consequence failure. After session loss, fresh authenticated reconciliation can prove a completed task and a validated durable snapshot digest. The requester retains that expected digest separately; it creates `result_identity` only from content actually received locally. The executor resends only its matching snapshot, never reruns the Agent. Lost final outbound Transfer acknowledgement, including a sender bookkeeping failure after receiver `/finish`, remains reconciliation-required and source-owning. Proven failure before Transfer finalization can become an ordinary interruption with best-effort cancellation of queued preparation.

If the source changed, Pastey retains the exact returned result without overwriting the source. `conflict_result_retention_required` and `conflict_result_retention_failed` remain explicit consequence-recovery states; exact Return repair or explicit abandonment remains available, and the old movement keeps source ownership while repair is admissible, including across requester restarts. A rematerialized result is matched by exact logical file-set content rather than inode or mtime. `WorkspaceApplyTransactionV1` journals the received result identity before filesystem changes. Restart may restore the baseline, complete a provably staged result, or recognize an already committed result; unexpected canonical state stays blocked and untouched. Exact duplicate Return does not reapply over later edits. Abandonment cancels movement authority without rewriting completed Agent history.

Reopening a Bridge projects one unresolved safe fact at a time into the existing card. Remote uncertainty offers Reconcile and Stop, durable Return offers Retry result Return, retained conflict offers Reveal result and Discard result, and consequence interruption can offer Abandon recovery. The projection excludes physical paths, native session/turn/process details, provider/auth state, result text, and reasoning/tool history. One canonical workspace remains owned while execution, movement, reconciliation, or apply recovery can affect it; only terminal resolution, explicit cancellation, or proven ordinary interruption releases that ownership. Session loss revokes Bridge transport authority but does not Burn the Bridge or terminate a still-observed Host-local turn. Burn removes Bridge authority and envelopes even when an apply journal cannot prove safe filesystem repair: canonical and uncertain sibling trees remain untouched, possible orphan residue does not block startup, and late observer writes cannot restore authority.

### Full Access

A remote invocation normally waits for the executing Host's Review. For a Host nobody is attending, its user can instead give the device a Bridge is paired with **Full Access** (`src-tauri/src/full_access.rs`): one decision that lets that device control this Host through Pastey without anyone here accepting each request. The use case is a Mac away from home driving a Windows PC no one is sitting at.

- **Device identity.** Each installation has one Ed25519 key (`device_identity.rs`), generated locally and stored wrapped with the app master key (not hardware-backed). A HostRef, display name, address or Bridge code is never treated as identity. A device proves its key only by signing the exact current session — Bridge, its own HostRef as signer, the peer's as verifier, the session pair derived from both ephemeral transport keys, and a fresh nonce — in a `native_agent.device_identity` Room Control event, which is itself authenticated to those transport keys. The receiving Host verifies it against its own current binding of the session it arrived on and remembers it only while that exact binding is current (process-local, at most 32). It cannot be replayed into another session or Bridge or presented as another Host. The requesting Host sends it before each remote invocation, workspace movement, session query and terminal request, and on "Show this device".
- **Who.** The device key and HostRef the Bridge's pairing code proved (`bridge_peer_pins`), over that Bridge, for this Host. The device need not be connected when it is given, since the pairing already proved its key; the consent dialog shows that key's fingerprint. Granting also trusts the key (`trusted_devices`), so it appears under Devices & trust. Only this Host's local command creates it (`grant_full_access`, which refuses unless the user confirmed what it allows); no Room Control event reaches it, and a test asserts `room_control.rs` names none of its mutators.
- **Direction.** It lets that device control this Host. It says nothing about this Host controlling that device, which only that device's own user can allow, there.
- **Lifetime.** Durable (`full_access_grants`): it survives reconnects and restarts. It counts only while one SQL condition holds (`FULL_ACCESS_IS_ACTIVE`): unrevoked, the device still trusted with exactly that key, the Bridge live and unburned, and the Bridge still pinned to exactly that key and HostRef. Revoking it, revoking the device under Devices & trust, forgetting the paired device, pairing the Bridge with another device and Burn each end it; trusting or pairing the same device again never brings it back. One active grant per Bridge.
- **Use.** A request counts as that device's only when its identity was proven in exactly the session the request arrived on. Then:
  - `native_agent.invoke` is admitted at once (principal `FullAccess` in `admission.rs`) instead of being held for Review. It is the same start as an accepted Review, as the same peer, with the same ownership, replay, exclusivity, conversation and requester-decision rules. A start the capability refuses (a folder another task holds, a thread another client has open) is a definite non-start, `native_agent_start_rejected`, which the requester is told.
  - A workspace sent by Workspace Movement is admitted when it lands, if the device proven in the session that prepared the movement holds Full Access then.
  - Every folder shared with that device allows Sessions with excerpts and Agent requests, whatever its switches say; the switches apply again if Full Access ends. The device may also add folders here by path (`OpenFolder`, below), which then count as shared folders for reading, running and continuing; this Host's user sees them under Bridge details → Full Access and can remove them. They pass the same folder rule as a share (no root, home folder, application data, hidden folder or link), are capped at 32, and are forgotten with the grant.
  - A Developer Terminal request opens without Accept ([architecture](../architecture.md#managed-workspace-and-developer-mode)).
- **Not covered.** It never answers a native capability's own approval requests; they go to the requesting device's user, as below, or wait until stopped. It never runs a capability local to its Host: Core refuses a Bridge invocation of any capability without peer protocols, so Claude Code continuation stays local. It does not touch the managed Plan path's Review and Host admission. It cannot pass the operating system: administrator and UAC prompts and macOS privacy permissions still apply. The Host must be on, awake, signed in and running Pastey; a locked session is untested. Revoking does not stop a Codex task it already started (Stop does), and it closes a terminal it opened.
- **What it replaced.** The earlier per-folder, run-limited unattended grants are removed on upgrade, never converted (`retire_unattended_grants`): a device that held one asks for Accept again until its Host's user gives it Full Access.

### Native approval requests

Codex blocks a turn on server-to-client requests (protocol facts in `capability/codex.rs`, checked against Codex `rust-v0.162.1`). Pastey never answers one by itself, by policy, by model output or because a permission started the task. It relays two kinds, one at a time, to the user of the device that requested the task:

- **Command execution** (`item/commandExecution/requestApproval` with `kind` `command`): the command, its folder, Codex's stated reason and any network host/protocol.
- **File change** (`item/fileChange/requestApproval`): the request carries no diff, so Pastey shows the changes of the exact `fileChange` item from this turn's `item/started` / `item/fileChange/patchUpdated`. Changes updated after they were shown can no longer be approved.

The executing Host builds a bounded review (`NativeDecisionReviewV1`): a Pastey-opaque `reviewRef` (native request, thread and turn ids never leave the Host), the material content as untrusted display text, and a BLAKE3 digest of exactly that kind and content. A task carries at most 3 reviews, each at most 10 KiB of content, in its running status; they are dropped when the task leaves Running (completion, Stop, Burn, session loss on the requester, observer loss, restart). Approval is offered only for a request of exactly this task's thread and turn whose content is complete, renders faithfully (no control, bidirectional-override or invisible format, separator or filler characters other than line breaks and tabs) and fits its bound, does not ask for additional sandbox permissions or a session-wide write root, and lists `accept` among its `availableDecisions` when it lists any. Otherwise only Deny, or nothing, is offered, with the reason. Permission requests, questions to the user, MCP elicitations, legacy v1 approvals and terminal-input approvals are shown as needing attention and are never answered; Stop interrupts the turn.

The only answers are `accept` ("Approve once": this one operation) and `decline` ("Deny": refused, the turn continues), sent as the JSON-RPC result `{"decision": …}` for the exact native request id. `acceptForSession`, policy amendments and `cancel` are never sent. The requester's `respond_remote_native_decision` re-checks its last status, proves its device identity in the current session, and sends `native_agent.decision` (`NativeAgentDecisionV1`: task id, target HostRef, `reviewRef`, content digest, decision and a per-attempt id; no command, path or native id). The executing Host's `submit_native_decision` accepts it only from the task's own Bridge (not revoked), the same peer HostRef and the same device key that requested the task, proven in the current session. A task whose requesting device proved no identity when it was created (including received-workspace tasks and tasks started on the Host itself) shows its requests without Approve or Deny (`native_decision_requester_unverified`) and takes no remote decisions, for a review that is still pending, unexpired (30 minutes), shown with that digest and offering that decision, while the task is Running and not being stopped. The adapter marks the request answered before writing, so a request is answered at most once; a duplicate, replay, late or stale decision is refused. Nothing waits for the user under the service lock.

Which path the Bridge session runs on (LAN, overlay or the internet carrier, direct or relayed) changes none of this: a path change is not a session change, and losing the path is a transport failure handled by the rules here ([Connectivity](layer-4-connectivity.md#transportsession-contract)). Nothing is queued for a device that cannot be reached.

The executing Host sends a status that did not reach the requester again, up to 5 more times. After session loss the requester treats the task as needing reconciliation and drops its reviews, so a request pending at that point can only be stopped, not answered. The review then reads `submitted` (given to Codex) and `resolved` once Codex reports `serverRequest/resolved`; neither claims what the operation did. Stop, Burn, session loss and restart never let a pending review resume execution.

### Direct remote invocation and explicit movement

For an existing workspace already present on a connected remote Host, Pastey
uses the same native session service through authenticated current-session Room
Control: exact selected Host, capability, opaque capability input (for Codex,
its workspace and task), task identity, and bounded lifecycle/status are
correlated end-to-end. Replay is rejected
before invocation and cancellation wins over a late status. This direct remote
case creates no ManagedObject, Scratch, Worker, GST scan, or Transfer.

Every task has one immutable owner: Local, or the Bridge it was created on,
recorded durably as the envelope's Bridge correlation. No invocation, replay,
workspace preparation, approval, or failed request from another principal can
adopt, move, or erase it, so Burn selects exactly the tasks its Bridge owns.
A Bridge can cancel only its own tasks, never a Local task or another Bridge's.
Workspace preparation is accepted only when its source Host is the exact
authenticated current-session peer `HostRef`. Ownership is integrity, not
admission: it does not decide whether a peer may invoke a capability.

Native executor admission is that decision, made by Core on the executing
Host before any capability adapter sees the input. It is separate from the
managed Plan path's attempt-bound Host admission. Bridge/session
authentication and capability availability are necessary but never
sufficient. The fixed policy admits a Local invocation at once, admits a
remote one at once when the device proven in exactly its session holds this
Host's Full Access (above), and otherwise holds the remote
`native_agent.invoke` for executor Review: after the exact authenticated peer `HostSessionBinding`, the
target Host, ownership/replay and availability checks pass, the task is
`queued` with `native_agent_review_required` and neither `prepare` nor `start`
has run. Accept is one-shot; it starts the invocation only if the exact
binding is still current, the Bridge was not Burned, the executor Review has
not expired (two minutes, bounded by the Bridge expiry), ownership is unchanged
and the capability is still available. Deny, expiry and lost authority end
the task `failed` with `native_agent_admission_denied`,
`native_agent_review_expired` or `native_agent_admission_revoked`; a
capability that refuses an accepted input ends `native_agent_start_rejected`.
A pending executor Review is process-local: it is never persisted, never exposes its
input to the renderer (only task, Bridge, peer `HostRef`/session, capability
name and expiry), ends on session loss, disappears on Burn or peer
departure, and is forgotten on restart. The one stored admission is Full
Access; there is no per-capability ACL or per-folder allowlist. Once started, an invocation keeps the existing lifecycle and
is not re-admitted on route or session change.

Received workspace movement goes through the same admission. Requester
movement approval authorizes moving the workspace and the remote operation;
it is never execution authority on the executing Host. Workspace preparation is
bound to the exact authenticated peer `HostSessionBinding`. When the encrypted
workspace lands and is durably owned by its movement, the task stays `queued`
with `native_agent_review_required`, still `transferring_to_agent`, holding the
landed tree and the requester's source; Codex is neither prepared nor
started. The same pending executor Review, projection, and Accept/Deny apply,
unless the device proven in the session that prepared the movement holds Full
Access when the workspace lands; then it starts at once. Accept
additionally revalidates that the landed workspace and held input still
belong to exactly that movement and task, then continues the existing
movement lifecycle and Return unchanged. Deny, expiry, session loss, and
restart end task and movement `failed` with the admission code and remove the
landed tree through the existing terminal cleanup, never through
reconciliation. Burn and peer departure remove them with the Bridge. A
duplicate preparation or landing creates no second executor Review.

When a workspace or result actually must move across Hosts, Pastey uses the
existing managed object/RegularFileSet and encrypted Transfer flow only for
that movement. Review is two-sided. On the requesting Host, the product
presents one movement Review in device terms—send the workspace to the Agent
Host, ask that Host to run Codex, then return the result—and requester
movement approval covers sending, the requested remote operation, Return, and
unchanged-source apply. On the executing Host, the landed workspace still
waits for that Host's own executor Review (above) before Codex starts; the
requester's approval is never execution authority there. The full order is:

```text
movement proposal → requester approval
  → workspace_prepare → encrypted Transfer → materialization
  → executor Review → Accept → adapter prepare/start
  → result → Return → source apply or conflict retention
```

The outbound receipt materializes as a
Host-private Pastey task workspace; Codex operates on that normal workspace
without seeing GST, receipts, source Host identity, or other movement internals.

After native success, Pastey scans the final task workspace solely because it
must return. The initiating Host rechecks the exact approved source baseline
before a bounded staged replacement. An unchanged source applies automatically
without another confirmation. A changed source retains the returned result and
enters `conflict_recovery_required` with a durable Host-private recovery copy;
it does not overwrite, merge, guess, or report movement completion. The Agent
task remains `Completed` if native execution completed. Before the requester's movement Review, a
cross-Host workspace must be representable faithfully: Pastey rejects
symlink/reparse and special entries, empty directories, executable modes,
invalid portable selectors, case-folding collisions, and bounded-manifest
violations. Agent failure, cancellation, interruption, transfer failure, or an
unknown Agent outcome similarly has no result apply and cannot prove completion.

### Native session discovery (read only)

Pastey can find the Claude Code and Codex sessions that already exist on this Host and report on them (`src-tauri/src/native_agent/sessions.rs`). It never starts, resumes, changes or stops a native session. It does not use windows, screenshots or UI automation.

- **Scope.** This Host's user names the workspaces, 1 to 8 of them. Each must pass the shareable-folder rule: a specific real folder, not a root, the home folder, an application-data location or a link. A session is in scope only when its recorded working directory is exactly one of those workspaces. The commands are local; no Bridge peer reaches them, and device trust grants no access to local conversations.
- **Claude Code.**
  - History is the session JSONL that the official Agent SDK reads (`listSessions()` / `getSessionMessages()`): `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/<encoded workspace>/<session id>.jsonl`. Only the folders of the named workspaces are opened, and each transcript's recorded `cwd` decides.
  - The desktop app's Code sessions and the CLI share this store. Entries say `entrypoint: claude-desktop` or `cli`.
  - Live state is `claude agents --json`, Claude Code's scripting list of active sessions. `busy` means running; any other listed status means a process holds the session idle; not listed means no process holds it. Without that command (not found, older version, timeout), live state is unknown.
- **Codex.**
  - History comes from Codex's own app-server, run as Pastey's read-only child from the Codex Pastey would launch (CLI or desktop app).
  - The reader may send only `thread/list` (exact `cwd` filter, state database only, so Codex repairs nothing), `thread/turns/list` and `thread/items/list`, in pages small enough for the line bound.
  - An app-server reports thread status only for threads it loaded itself. Who has a thread open is answered, in order, by: Pastey's own app-servers, named by thread id (Pastey's conversation in a folder, idle or running a task, and any continuation in flight); Codex's per-thread writer lock where it can be read without taking it (macOS, below); otherwise the process list, which is device-wide: no Codex process other than Pastey's own children means no client can have any thread open, and any such process makes it unknown for every thread.
- **Status.** Live facts decide execution; history only explains how a session nothing runs had ended.
  - Running and idle come only from a live source.
  - With no live process: the last recorded turn gives completed, failed or interrupted. A history that stops mid-turn becomes interrupted, as an inference.
  - With no live source: unknown, whatever the history says.
  - A completed turn is not a completed task.
- **Resume verdict.**
  - `safe`: nothing holds the session, its workspace exists and its history is readable.
  - `unsafe`: a live process holds it.
  - `unknown`: otherwise.
  - This is an assessment only: nothing resumes.
- **Snapshot.** For one session: activity counts, the newest prompts and replies, and read-only Git facts about the workspace now.
  - Every statement carries its basis (`fact`, `inference` or `unknown`) and source.
  - The agent's replies are labelled `agent_claim`, and the snapshot states that the work they describe is not verified.
- **Choosing a session.** Provider, text (title, workspace folder name, branch) and recency narrow the list. One match is `unique`; several are `ambiguous` and returned as candidates, newest first. Pastey never picks among them.

#### Sessions, processes and tasks

What Pastey means by each, and which part of Pastey answers what. The boundaries matter because each answer comes from a different source with a different reach.

- **A session is saved history, not a process.** A Codex thread is Codex's own record in that device's Codex home (`~/.codex`, its state database and rollouts); a Claude Code session is its transcript under `~/.claude/projects`. It belongs to one device and to the folder it was started in, and it outlives every client: quitting the Codex app or a terminal ends a *process* that had the thread open, not the thread. That is why Pastey still finds a thread after every Codex client on that device has quit, and why it could continue one in the real Mac → Windows acceptance after the app was closed.
- **Four responsibilities, kept apart:**

| | Answers | Source | Reach | Changes anything |
|---|---|---|---|---|
| Discovery | Which sessions exist in this folder | Codex: Pastey's own read-only `codex app-server` child, `thread/list` with an exact `cwd`, state database only. Claude Code: the transcript folder | The named or shared folders on that device | No |
| History inspection | How its last turn ended, its end marker, counts, excerpts | `thread/turns/list`, `thread/items/list`; the transcript | One session | No |
| Hold observation | Whether something has it open now | Pastey's own holds; Codex's writer lock (macOS); else the process list (device-wide); `claude agents --json` | A moment: it can change right after | No |
| Continuation | One new turn in the session itself | `agent.coding.codex_thread` (`thread/resume` under Codex's lock) or `claude --bg --resume` | That session, once, after the Host's Accept (or under its Full Access) for a peer | Yes, once |

- **Discovery and inspection never decide continuation.** Their verdict (`resume`) explains in advance; the decision is taken again at launch, by the provider's own exclusion (Codex refuses a second writer; Claude Code copies a held session, which Pastey stops and reports as unknown) and by Pastey's check that the session still ends with the marker the user saw.
- **Pastey's own holds are named.** Pastey keeps its own conversation's app-server open between tasks in a folder (New task), so its writer lock is held while idle. Discovery reports that thread as "Pastey's own Codex session" (idle: `attached`, resume `safe`; running a task: `running`, resume `unsafe`), never as another client's. A continuation of that thread first checks, in its own app-server and without loading the thread, that Codex records the thread for exactly the admitted folder (`thread/list` with an exact `cwd`, state database only), and only then closes Pastey's *idle* session of it in that same folder (`capability/codex/own_holds.rs`; nothing is lost: the conversation is saved and reopened with `thread/resume` next time). It waits up to 5 s for that app-server's lock to go (on macOS by testing the lock, since Pastey kills the app-server and Codex may leave the file; elsewhere by the file's presence), and then relies on Codex's own refusal as before. "Idle" is the Codex adapter's own occupancy flag, the one its session trimming uses: it is set under the session store's lock when a task takes the session and cleared only when that task's observer is done with it (for an unknown outcome, only after the app-server's channel is lost), so a session running a turn, waiting on a native approval or holding an unknown turn is never closed; that continuation is refused before anything runs. Folder exclusivity keeps a Pastey task from starting in that folder meanwhile, and the observer lets go of a run (its occupancy, or a continuation's own app-server and lock) before it releases the folder.
- **How each state is said.** The renderer explains each live source (`src/lib/nativeSessions.ts`, `sessionHoldLine`): the writer lock is a fact about this thread; the process list is a fact about the whole device ("no Codex app or terminal is running on Windows, so nothing has it open"; or "Codex is running there and Pastey cannot tell which threads it has open: if this one, Codex refuses and nothing runs"). The verdict is shown as what the last look found, never as a promise.
- **When things change.**
  - A client opens the thread after the look: Codex refuses at `thread/resume`; the task ends "Did not start · nothing ran" with that reason and the fix.
  - The session changed: refused under the lock; "look at it again".
  - The device disconnects before Accept: the request is not delivered (a definite non-start) or its outcome is unknown, by the existing rules; after Accept, the existing reconciliation applies. Nothing is retried.
  - The share or a switch changed, or the Bridge session changed, before Accept: "access ended before it started · nothing ran".
  - History unreadable: not continuable. Codex without writer locks (before 0.163): refused at launch.

### Remote session reading

From one device, the user can see the Claude Code and Codex sessions on another connected device without that device's windows. The sessions card in the Bridge workspace is labelled "Agent sessions"; the user picks the device. The answer is read only. Wire facts are in the [reference](../reference.md#remote-native-session-reading-facts).

- **Permission: a workspace share.** The device that holds the sessions decides, inside the Bridge: Bridge details → Workspaces → Share a workspace…. Its user picks one folder with that device's own folder picker; the share is for the Bridge's own device, which must already be trusted (an inline Trust… with the fingerprint is offered there, once). For 1 hour to 30 days, each share turns on separately:
  - **Sessions**, with or without short excerpts;
  - **Agent requests**: the other device may ask to run Codex there, or continue a Codex thread there when it may also read sessions. Each request waits for this device's Accept, unless its user gave that device Full Access.
  - Sharing a folder allows nothing until one of these is on, and trusting a device shares nothing.
  - No Room Control event creates, changes or ends a share. The peer sees a folder only as its name and a per-Bridge reference, never its path.
  - Stopping the share, revoking the device, Burn, expiry, or moving the folder ends it. Settings → Permissions → Shared workspaces lists every share across Bridges and can stop any.
- **Full Access.** A device the Host gave Full Access may read every folder shared with it, with excerpts. It may add a folder by its full path: `native_agent.session_query` with `operation: open_folder` and `folder`, refused (`folder_refused`, with the Host's reason minus any path) without Full Access or for a folder too broad to share. Every answer carries `fullAccess: true` while the device has it, and a folder allowed only by Full Access has no `expiresAt` and carries `fullAccess: true`. The requesting device's Agents view then shows **Workspaces on <device>** with an Add a folder… button.
- **Asking.** The query names the Host and optional filters: provider, text, recent activity and one shared folder. It travels over the exact current peer session.
  - The Host checks the device identity proven in that session against its permissions.
  - It runs the same discovery as locally, only in the permitted folders.
  - It answers without folder paths. Titles, statements and excerpts lose any path-like text, and changed files are named only relative to the folder.
  - Excerpts are sent only when permitted, and never as whole transcripts.
- **Showing.** Agents lists the sessions of the selected workspace by itself, once per workspace, when that device shares it with Sessions on (or for a folder on this device); a refresh repeats it. Each row has provider, branch, origin, last activity, status and whether it can be continued from here. Pastey never picks one. A session opens as a conversation: its messages (this device: the newest 40, up to 4,000 characters each; another device: the excerpts it shares, at most 4 of 300 characters), events between them, this device's turns in it, and a composer that continues it. Details on request: its state and who has it open (with how Pastey knows), activity counts and changed files, Git facts about the folder now, the live source and when it was checked, how the last turn ended, its ids, and the evidence split into facts, inferences and unknowns.

  A session is never shown as running or done because of its last message.
- **Windows.** Discovery uses the plain form of a canonical Windows path for Claude Code's folder name and for Codex's `cwd` filter, and for relative changed files. Codex's live state comes from a `Win32_Process` query for `codex.exe` (Codex's writer lock cannot be read there without taking it). Mac → Windows discovery, inspection and continuation of a Codex Desktop thread passed on real devices (October 2026): with the app open the thread was refused by Codex's writer lock and nothing ran; with every Codex client closed it was found, continued in place (1 → 2 turns), and released. See the [acceptance procedure](../platform/mac-windows-acceptance.md).

### Native session continuation

This Host's user can continue one discovered **Claude Code** session with a new prompt (`continue_native_session`, adapter `agent.coding.claude_code` in `src-tauri/src/native_agent/capability/claude_code.rs`). The task runs on the ordinary Native Agent lifecycle: task identity, local admission, one invocation per session (exclusivity), status, Stop, restart handling and release.

- **Mechanism.** Only Claude Code's own `claude --bg --resume <id> -- <prompt>`.
  - Measured on Claude Code 2.1.292: it continues the session under its own id only when no Claude Code process holds it. When a background or foreground process holds it, it starts a copy and says so, and the original is unchanged.
  - A headless `claude -p --resume`, by contrast, wrote a held session concurrently: its history branched.
  - Pastey passes the prompt and nothing else, because a resumed background session given other options is also copied.
- **Checks.**
  - The command first requires discovery's resume verdict `safe`.
  - The adapter then checks again, immediately before launching, that the transcript is the workspace's and that `claude agents --json` lists no process holding the session. Otherwise nothing is sent (`native_agent_start_failed`, with the reason).
- **Outcomes.**
  - Continuing under the same id is followed through the registry (`busy`, then `idle`) and the transcript.
  - The task completes only when the transcript records a normal end of turn after Pastey's prompt. The agent's final reply becomes the task output.
  - Pastey then runs `claude stop` (the conversation is kept), so the session is free again.
  - If Claude Code started a copy, Pastey stops the copy and the outcome is unknown: the copy may have begun the prompt.
  - An answer Pastey does not understand, or no answer in time, is also unknown.
  - A refusal before launch (for example an untrusted workspace) is a definite non-start.
  - Nothing is ever sent twice.
- **Approvals.** The session keeps its own permission settings. A registry status other than `busy` or `idle` is shown as waiting for a native decision. Pastey never answers it; Claude Code's own clients do (`claude attach`). Stop runs `claude stop`.
- **Local only.** The adapter advertises no peer protocol and is left out of the capability facts sent to peers. Core refuses a Bridge invocation of any capability without peer protocols, so reading or discovering a session never gives a peer write control of it.
- **Every continuation names the entry its user saw.** Discovery reports each session's `lastTurn.marker` (Claude Code's last main-chain entry `uuid`, Codex's newest turn id); `continue_native_session` refuses when the session no longer ends with it.

#### Codex threads (`agent.coding.codex_thread`, `capability/codex/thread_continuation.rs`)

Since Codex 0.163 every app-server that loads a thread holds an exclusive lock on `<CODEX_HOME>/thread-writer-locks/<thread>.lock` and removes it on exit, and another app-server's `thread/resume` of that thread is refused ("already has an active writer"). Measured on macOS with the CLI and with the desktop app's bundled Codex, which refuse each other ([codex-session-probe](../../tools/codex-session-probe/README.md#update-2026-10-19-codex-0163-thread-writer-locks)). Pastey builds on that, and on nothing private:
- **Observation.** Discovery reads the lock without taking it (on macOS a POSIX `F_GETLK` reports the flock): held is "another Codex client has it open" (running when its last turn is in progress), free is a fact that none does. On macOS a missing lock file is also free (Codex was measured to lock exactly that file). A thread Pastey itself has open is reported as Pastey's before the lock is read. Elsewhere (Windows, Linux) the lock cannot be tested without taking it, and where Codex keeps it is not measured, so neither an existing nor a missing lock file is taken as evidence: the process-list rule applies. None of these observations is consulted when a continuation starts; only Codex's own `thread/resume` refusal is.
- **Continuation.** Each run opens its own app-server, used for one turn and then closed, so the lock is held exactly while Pastey's turn runs. Before taking anything it checks that Codex records the thread for exactly the admitted folder; a peer's thread id is its own claim, so a thread of another folder is refused before it is loaded. If Pastey's own conversation has the thread open, idle, in that folder, that session is closed next (above). `thread/resume` takes the lock or is refused (a definite non-start that says the thread is open in the Codex app or a terminal). Before anything else, the app-server Pastey launched must report (in its `initialize` answer's `userAgent`) a Codex version measured to keep writer locks, 0.163.0-alpha.10.2 or later; an older or unreported version is refused, because it would resume a thread whatever lock another client holds, and a lock file seen afterwards could be that client's or one left behind. Holding the lock, the adapter checks that the lock file exists, that the thread's recorded folder is exactly the admitted one, that its newest turn is still the marker and not in progress. Then one `turn/start`, tagged with the task id as `clientUserMessageId`, observed like any Codex turn (native requests, Stop, response). Every refusal precedes `turn/start`; a thread is never replaced by a new one, and nothing is sent twice.
- **Unknown outcomes.** An unknown turn's app-server is closed rather than left running unobserved; the outcome stays unknown, and the lock is released. If Pastey stops, its app-server ends with it.
- **From another device.** The adapter advertises the generic invocation protocol. The requesting device sends the thread id, prompt and marker, and names the folder only by the `scopeRef` of the folder's share. The executing Host resolves that scope among the folders it shares with that exact device with both Sessions and Agent requests on (`session_read_access::resolve_agent_request`, from `room_control`, before admission); a path or any other scope is refused. Every such invocation waits for the Host's Review, unless the requesting device has Full Access there. The Review names the Host's own shared folder it is for (`NativeInvocationReviewV1.sharedFolder`, a folder name shown only on the Host), never the request's content. Reading never implies continuing; continuing needs reading, agent requests and an Accept, or Full Access.

#### Running in a shared folder

Run with Codex on another device offers **Shared by <device>**: a folder that device shares with this Bridge with Agent requests on, chosen by name. The v1 invocation carries the folder's reference in `workspace`; the executing Host resolves it from its own share (or, under Full Access, a folder that device added) before admission (above) and refuses a reference it does not hold for that device. The path is never sent, and the run is otherwise the same: same Review (or Full Access), same conversation (keyed by the Host's own folder), Stop and reconciliation. Typing a path on that device (**Another folder on <device>**) and sending a folder from this device (**On this device**, Workspace Movement) are unchanged.
- **Limits.** Desktop UI and the desktop's private app-server stay untouched: Pastey cannot see or steer a turn the Codex app runs. Every Codex client on the Host must be 0.163 or later. Windows and Linux enforcement is inferred from Codex's shared implementation, not measured.

Remote **Claude Code** continuation stays off ([remote-continuation](../../tools/native-session-handoff/remote-continuation.md)).

Observing or taking over a turn that is already running is research only; see [native-session-handoff](../../tools/native-session-handoff/README.md).

## Managed execution path

The Generic Managed Worker is a separate, currently implemented path for exact managed Plan steps. Its semantic workspace and `ManagedObject` revision flow apply to this path; they are not the definition of every Agent capability. The following roles and contracts describe managed execution unless stated otherwise.

## Responsibility and locality

| Role | Canonical responsibility |
| --- | --- |
| PM | WHAT / WHERE / ORDER. It may propose semantic structure, but has no effect tools and no direct approval or start authority. |
| Worker | HOW for one already-authorized and claimed step. It cannot choose Host, topology, Transfer, approval, grants, lineage, or successor dispatch. |
| Core | Identity, topology, authority, admission, effect enforcement, evidence, result, lineage, and authoritative completion. |

PM and Worker may propose or request. Only Core authorizes and finalizes authoritative state; DONE and authoritative completion belong to Core, not PM.

Local execution is a transport/dispatch optimization, not an authority exception. Core resolves the authored participant's `HostRef`; the local path then uses direct in-process coordinator actions instead of sending Room Control messages to itself. It still performs exact Review, readiness, attempt-bound Host admission, prepared/commit, one-step claim, effect enforcement, evidence, result acceptance, step commit, continuation, and cancellation. Locality never bypasses Core authority.

Local admission is bound to the active Bridge, exact requester participant and `HostRef`, one opaque `LocalRuntimeRef`, and expiry. `LocalRuntimeRef` identifies only the current HostRuntime generation: it has no peer, route, Bridge-session, or `session_pair_ref` fields. Restart creates a different value and invalidates the old local authority even though the durable `HostRef` is unchanged. Remote admission continues to require the existing exact `HostSessionBinding`.

## Current managed semantic model

Search / Transform / Transfer / Execute are the current managed primitives, not universal verbs for future Host capabilities. A task may execute where its needed capability and resources already reside. In a managed Plan, explicit Transfer is needed when an authored step requires its exact object revision on another Host.

| Primitive | Contract |
| --- | --- |
| Search | Find exactly one bounded object on an explicit Host and bind the declared logical revision. |
| Transform | Apply reviewed modification intent to exact revision N at its current Host and propose same-object N+1. |
| Transfer | Move the exact current revision from one authored Host to another without changing its logical revision. |
| Execute | Run the exact current revision on its authored Host and produce a result digest, never lineage. |

**Transfer is the only current managed primitive that changes ManagedObject location.** A Transform cannot select a new Host or produce an unrelated logical object. An Execute cannot create a revision. Every movement, dependency, mutation intent, execution intent, and Host is part of the sealed Plan.

Transform is generic across representations. A Python→Java conversion may be used as an acceptance example, but it is not a Python/Java subsystem, alternate execution path, or additional primitive.

## Plan v1 and v2

V1 remains the compatibility product path. It uses a Search-first `selected_file` flow, executes Search and Transfer, and rejects Transform/Execute as unsupported. V1 schemas, hashes, protocol, approval, and continuation are not reinterpreted by v2.

**Find file** (the Share composer's + → Get a file from <device>…) is the product entry to that v1 flow: the requester composes `Search @ selected device → Transfer to requesting device (final delivery, Inbox)` with a filename hint, an optional extension, and `authorized_folders`; reviews and approves that exact revision; the selected device searches only those places and returns bounded candidates (name, size, modification time, redacted location); and only the one candidate the requester chooses is revalidated and sent through the existing encrypted Transfer to the requester's Inbox. It never starts workspace movement, Return, or apply. Another device searches only `authorized_folders`: the folders the searched Host's own user shared, each with exactly one Bridge, under Settings → Permissions → Shared search folders (system folder picker, local command only; at most 16; roots, the home folder or its ancestors, OS application-data and cache locations, Pastey's data, hidden folders, and links are refused). A Search reaches only the folders shared with the Bridge it arrives on; another Bridge, even between the same devices, reaches none. The burned-Bridge tombstone ends a share at once and finalization deletes it; a folder shared before Bridge scoping is never searched until shared again. No path ever crosses the Bridge, and nothing a peer or an Agent sends can share a folder; a filename an Agent mentions is at most a search hint. Search re-canonicalizes each shared folder and skips one that was moved, deleted, or replaced through a link. A candidate is bound to its Bridge, requester, receiver, attempt, scope, and the exact file object found (volume/file id, size, times); selection fails if it expired (10 minutes), its folder stopped being shared, or the file was replaced or changed. Matches are ranked newest first: one Search visits at most 20,000 entries (depth 8, 10 seconds) and returns the 20 most recently modified matches it saw. The searched Host refuses its built-in places (`downloads`, `desktop`, `documents`, `pastey_shared`) for any Search a peer requested, whatever an approved revision names, and a candidate from a built-in place is never handed to a peer; only a Search this Host's own user requested on itself (its own v2 Plan, such as the Bridge Device Check) may use them. There is no default remote access, and a Host with no shared folder answers `no_searchable_scopes`. The manual composer, Find file and the deterministic natural-v1 proposal offer only `authorized_folders`; a built-in place that a provider-written proposal names is refused by the searched Host like any other. The chosen file is then opened once, without following links and only inside its scope, and sent only if it is still exactly the selected file object; Transfer reads that descriptor, never the path again, and fails before finishing if the file changed while it was read.

Native v2 uses durable `HostRef`-backed participants, generic `PlanRootV2` managed roots, explicit dependencies, explicit Hosts, and `ManagedObjectRevisionV2`. Roots can represent an already acquired Inbox item, drag/drop selection, local selection, Search result, or generated artifact. Acquisition is not a fifth primitive: the Host privately validates a physical artifact and binds it to an exact logical object/revision.

Core validation tracks the one current location and revision of every logical object:

- Search declares its output at its authored Host;
- Transform must consume the exact current revision at that Host and declares N+1 there;
- Transfer must consume and output the same revision while changing only the authored location;
- Execute must consume the exact current revision at its Host;
- each consumer depends on its exact producer;
- missing, hidden, or inferred movement fails validation.

The canonical B→C rule is therefore structural: a Transform result N+1 at B cannot be consumed at C until the authored B→C Transfer completes and C has the exact receipt.

## Managed Plan Review, approval, readiness, and Host admission

The deterministic native-v2 Composer accepts only explicit HostRefs, roots, and authored steps. It sorts participants, validates the dependency/object flow, and seals one immutable revision/hash. Natural-v2 may feed this Composer only after Core resolves its bounded aliases; neither path approves or starts the result.

One requester approval binds the complete immutable revision. Attempt start then follows a fail-closed distributed barrier:

1. The requester resolves locality from the exact authored `HostRef`. Its own participant captures the current `LocalRuntimeRef`; every remote participant asks the canonical Layer 4 resolver for exactly one transport-proven current session and consumes its `HostSessionBinding`. Layer 5 does not interpret Bridge peer rows, reconnect markers, endpoints, or transport keys.
2. Each Host validates the complete immutable Plan and its exact participant/freshness correlation, then evaluates roots, transfer counterparts, provider generation/model, Host-selected runtime resolution, process binding, and verified platform world only where required by its own authored fragment.
3. Any Host-local requirement reported unavailable fails the whole Plan before an earlier Search, Transfer, or managed step can execute; availability on another Host cannot satisfy it.
4. Each bound Host validates the exact review correlation and creates Host admission in prepared state. Remote Hosts receive authenticated protocol messages and claim their replay identities; the requester invokes the shared semantic lifecycle through direct typed coordinator actions and creates no local protocol replay claim.
5. Only after every Host is prepared does the requester send commit; receivers execute nothing before it.

Search/Transfer-only Plans do not require a Worker provider. A resource-only Transform requires a provider but not a process world. A process-backed Transform and every current Execute require a Host-private exact revision/step process binding and a verified execution world.

Review, readiness, prepared, commit, result, failure, step-commit, and cancellation transitions carry the exact Plan, revision/hash, approval, attempt, participant, Host freshness, TTL, and correlation appropriate to the transition. For remote Hosts, the full directional `binding_ref` remains Host-private authority and cross-side readiness/prepared/result/failure messages use the symmetric, non-authoritative `session_pair_ref`, derived from the exact Bridge and both Host/session endpoints independent of direction. Local transitions instead carry the exact local-runtime freshness internally and never fabricate a remote binding or session pair. Replays and substitutions fail closed.

## Step and effect authority

After commit, the Host coordinator atomically reserves one authored, dependency-eligible local Transform or Execute. Its durable dispatch row and Core managed-step claim are one-use. Duplicate dispatch, changed Host/session/provider binding, terminal attempt state, or a late result cannot create a second run or unlock a dependency.

Core derives `StepWorkDescriptorV1`, `AuthorityContextV1`, one `ManagedRunRefV1`, and an `EffectEnvelopeV1` for that exact claim. Tool visibility is not authority. Every request is lowered to an `EffectRequestV1` and revalidated against the active run, envelope, sequence, budgets, handles, session, and backend availability.

### Resource

`ManagedRunWorkspaceV1` is the Host-private workspace/lifetime aggregation for one exact active run. It is derived from the existing authority context, EffectEnvelope resource attachments, effect bounds, budgets, optional execution-world binding, and current lifecycle state. It mints no grant or permission. The ABI distinguishes managed input, workspace/overlay, output, and scratch roles. The current live claim projects its exact immutable input, a Transform OutputSlot where applicable, and a run-local Scratch resource for a process-backed step so both macOS and Windows have a writable ephemeral working-directory role. Workspace/overlay remains an available Host-private resource role but is not yet added to the current Harness claim.

`WorkerWorkspaceProjectionV1` is the bounded model-visible view. It exposes only logical aliases, resource roles, the operation vocabulary that is present in both the resource grant and current envelope bounds, and whether relative selectors are meaningful. It contains no Host path, Host/session identity, raw resource handle, safe physical identity, EffectEnvelope internals, credential, Bridge topology, or resource from another run or Host.

Every Worker resource request resolves `alias + relative selector → current run projection → Host-private resource attachment → existing Resource Effect enforcement → backend`. Resolution rejects a stale or substituted projection, alias/role mismatch, absolute or escaping selector, revoked run, changed Host/session, and cross-run handle before effect dispatch. The EffectRequest still requires the exact current envelope, run sequence, budgets, resource grant, Host/session authority, and backend enforcement; the projection never authorizes an effect.

Managed input revision N remains immutable. Mutations stay in existing private overlay/output mechanisms, Scratch cannot become lineage, and output sealing alone creates no lineage. Only Core may accept a sealed Transform result. Cancellation, disconnect/session invalidation, Burn, shutdown, restart, run termination, and authoritative completion revoke the underlying attachments, so a prior projection cannot be reused. A step on another Host receives a new Host-local workspace projection; no workspace state is inherited or transferred.

`ExecutionWorld mount lease != exclusive Host resource lease`. Every resource mounted into an ExecutionWorld remains world-leased, so duplicate or reused world leasing is rejected. Only writable/private-overlay mounts are resource-exclusive while the world owns them. An immutable read-only `ManagedRevision` may therefore have a read-only ExecutionWorld projection while an authorized Host/Worker Read coexists. Writable Scratch, OutputSlot, and other writable overlays remain exclusive against conflicting Host resource effects while leased. This is platform-independent Pastey resource semantics: it preserves exact identity, envelope/grant ownership, selector validation, budgets, immutable ManagedRevision behavior, writable-overlay isolation, and fail-closed enforcement.

### Process

The current model-visible process catalog exposes one `process_spawn` request only when Core has prebound an exact executable identity and execution-world specification to the exact revision/step. The model cannot choose an executable, raw shell, Host path, ambient environment, cwd, network policy, or terminal. Pure lowering produces the existing `ProcessEffect::Spawn`; signal/termination remains Host lifecycle authority and is not a general model tool.

For current Execute steps, the Host configuration service accepts only the logical `python` or `node` runtime identity, discovers it from a bounded platform-specific set of absolute locations, and pins the resulting safe executable identity in Host-local SQLite. It stores the physical path only in that Host-private configuration. When an immutable revision reaches readiness, the target Host revalidates the pinned identity and calls the existing `bind_v2_managed_process_step()` for each Execute authored to itself. The process-world spec carries that private identity so both readiness and claim reject replacement in place. A missing selection, missing/replaced binary, unsupported identity, or wrong-Host step leaves readiness unavailable. Capability probes, the requester, PM, Worker, and provider cannot populate this binding; there is no PATH fallback, automatic installation, alternate executable substitution, new execution engine, or new authority domain. Resource-only Transform remains process-free unless a separate exact binding already exists.

The separate bounded Host capability probe is diagnostic-only. Its fixed-probe request vocabulary is global, while `RUNTIME_PROBES` is the receiving Host's platform-local implementation map. A recognized request with no local probe is a `system_probe_unsupported` observation; a failed local fixed probe is `system_probe_unavailable`. Neither outcome proves absence or supplies, discovers for execution, pins, or binds an executable identity. Managed execution continues exclusively through `ManagedRuntimeConfigServiceV1` selection, exact identity validation, existing readiness/admission, `ExecutionWorld`, and Core completion.

`ExecutionWorldServiceV1` owns the generic execution semantics: it validates exact authority and resource leases, uses mutable overlays, applies Pastey wall/output/write budgets and available platform observations, records evidence, owns cancellation, and waits for an observed terminal state before run revocation. It delegates only platform world preparation, process launch, standard-I/O transport, termination requests, and platform observations through `PlatformExecutionBackendV1`. The backend receives already-authorized mounts and launch data, mints or widens no authority, and has no unsandboxed fallback; unavailable preparation or launch fails closed.

Every available backend must attest to `AuthorizedResourceProjection`, `AuthorityNeutralEnvironment`, `ExplicitProcessIo`, `PlatformSandboxedProcess`, `CancellableProcessSession`, and `NoRawNetwork`. Authority-neutral means no authority-bearing state is introduced, not that every operational environment value is absent. Cancellable means Core can request termination through the backend and observe a terminal state; stronger descendant-destruction or resource-accounting claims require separate platform evidence.

Windows uses `WindowsCodexBackendV1` to implement this platform seam over a Codex-derived sandbox. It receives the exact executable, invocation, working directory, environment, and resource roots already authorized by Core. Availability requires Host-owned setup and native conformance; setup or launch failure has no unrestricted fallback. See [Windows managed execution](../platform/windows-managed-execution.md) for the platform semantics and truthful limitations. macOS is available only when its local confinement probe succeeds; Linux remains unavailable.

### Network

`NetworkBrokerServiceV1` is an independent Host-owned TCP/DNS authority domain outside the execution world. It requires its own scopes, budgets, revalidation, closure, and evidence. The Worker catalog does not expose it. Provider HTTPS is control-plane transport, not a `NetworkGrant`, task effect, or reusable egress channel.

Developer Terminal uses a separate grant/type/store/lifecycle and can never satisfy an EffectEnvelope or process binding.

## Capability observation and acquisition confirmation

The Bridge-native NodeList and bounded Host probes are complete observation foundations. `Available`, `Unavailable`, `Unsupported`, and no observation remain facts only: capability is not authority, probe availability is not executable binding, and execution-side exact Host binding remains separate.

Low-friction Capability Acquisition confirmation foundation — DONE

Generic capability acquisition/install behavior — not implemented

Acquisition intents use the generic bounded semantic-ID syntax rather than the fixed-probe vocabulary. For example, `runtime.java`, `tool.cmake`, `sdk.android`, and `model.whisper` are valid acquisition intents, while `runtime.java` is rejected by the fixed Host probe path until that vocabulary and a fixed implementation support it. The renderer-safe request carries the exact durable `HostRef`, semantic ID, and bounded display facts only. Confirmation returns only `Confirmed` or `Cancelled` and creates no installer behavior, probe, process binding, execution authority, Plan/topology mutation, Host selection, or Developer Mode change.

Native Agent integration does not provide a generic installer or capability-acquisition authority. Any future Host-side acquisition action must flow back through the existing probe → capability projection → NodeList/Settings refresh path. Confirmation itself performs none of those stages.

## Worker Harness

The Worker owns HOW for one already claimed step. It never owns WHAT, WHERE, ORDER, Host/topology selection, approval, admission, grants, Transfer, lineage, successor dispatch, raw filesystem/process/network access, or Developer Terminal.

```text
StepWorkDescriptor + bounded resource/semantic projection
  → TurnAssembler + WorkerSessionLog
  → ProviderAdapter normalized streaming turn
  → validated WorkerToolCall
  → WorkerToolCatalog pure lowering
  → existing Phase 5 effect enforcement/backend
  → authoritative EffectEvidence + bounded structured observation
  → next model turn
  → StepResultProposal
  → Core-only finalizer
```

`TurnAssembler` builds stable Worker instructions, the semantic step operation and intent, the bounded `WorkerWorkspaceProjectionV1`, schemas derived from that projection, ordered observations, and the operation-specific completion template accepted by `WorkerProviderResponseV1`. `WorkerProviderRequestV1` is the model-visible Worker Context Contract, and the OpenAI-compatible adapter transmits its step, workspace, bounded history, completion template, and tool schemas. The provider receives only a read-only cooperative cancellation token, not the run's Bridge/session correlation. `WorkerToolCatalogV1` resolves the existing inspect/read/create/replace/process calls only through the Host-private workspace aggregation before lowering them to the existing effect boundary. It never discovers an ambient repository or injects full topology, paths, handles, grants, credentials, or terminal data. `WorkerSessionLog` is process-local model-visible history, not an authority record.

The provider-neutral adapter accepts only the exact configured response model (until a Host-owned alias qualification exists), and records that bounded returned identity only as diagnostics. It assembles indexed tool calls with a stable id and `function` type; repeated id/type fields may be omitted but may not contradict, while fragmented names and arguments append deterministically. `parallel_tool_calls` is explicitly disabled and any multiple, missing-identity, partial, malformed, interrupted, stale, or cancelled stream produces no effect. Provider error bodies are bounded and sanitized; rate limits use a capped `Retry-After` or capped exponential backoff on the same immutable provider/model binding only.

Observations are bounded/redacted feedback. Resource observations carry operation/status, safe metadata, bounded content or digest, and truncation. Process observations carry exit status, stdout/stderr excerpts and digests, truncation, duration, and bounded facts such as network denial. They never replace `EffectEvidenceV1`, which remains the authoritative ordered Core record.

Provider sampling and context overflow have separate bounded retry policies. Retried provider sampling waits in cancellation-observable slices and never selects another provider or model; context overflow may take only the existing one-compaction path. Compaction keeps tool-call/result pairs together and changes only model-visible context. A failed deterministic tool strategy may be observed and corrected while the same run remains active. Cancellation, malformed output, terminal provider failure, ambiguous/interrupted effect state, or an indeterminate effect is not retried as a fresh effect.

### Worker Context Contract

The minimal Worker Context Contract is implemented at the existing `WorkerProviderRequestV1` seam rather than as a new subsystem. For one claimed Transform or Execute, the model receives only the semantic operation and intent, bounded workspace aliases/roles/operations/relative-selector facts, the corresponding semantic tool schemas, bounded history, and the exact operation-specific final-response template. Numeric input revision, approval lifecycle wording, physical bindings, raw filesystem paths, whole-Plan topology, sessions/routes, credentials, Host-selection data, and authority handles remain outside the provider payload. Internal revision correlation remains unchanged in Core/Host authority and finalization.

GST-1 extends the Host-private managed revision binding without changing `ManagedObjectRevisionV2`: an authoritative same-Host revision may be either one regular file or one bounded regular-file-set rooted at a private directory. A file set is a canonical ordered set of normalized relative selectors with per-file digest/byte facts and a Host-derived aggregate digest; symlinks/reparse points, special files, empty directories, executable modes, arbitrary metadata, and ambient filesystem state are not represented. Worker inspect/read remains alias- and selector-based, and `final { output_selector: "." }` seals the complete tracked OutputSlot file set before Core registers N+1.

GST-2 completes exact authored regular-file-set Transfer without adding a Plan or Layer-5 primitive. The sender revalidates the complete bound tree and frames its canonical selectors, per-file logical digest/bytes, aggregate digest, and bytes into one bounded Host-private package that uses the existing encrypted single-file Room transfer. The receiver validates and materializes that package into a fresh private tree, canonical-rescans it, and only then binds the same logical object/revision as the Transfer receipt. Package bytes and path are transport-private and never become a managed object; the existing canonical regular-file-set digest, not a package digest, remains authoritative. Scalar Transfer still sends the original regular file directly.

GST-3 completes RegularFileSet Execute without a new primitive or execution subsystem. The existing `ManagedRevision` ExecutionWorld mount resolves the exact acquisition and canonically rescans a file set before lease and after release; it mounts the Host-private tree root read-only, while the existing semantic `input` alias with selector `"."` resolves the contained process cwd to that root. The existing exact Host-private executable binding, one `process_spawn`, raw-network denial, bounded evidence, and Core finalizer remain unchanged. Execute yields only its result digest: it accepts neither a changed tree nor an N+1/successor lineage. The representation and physical path remain outside Worker context and provider payloads. Native mature Agents are not GST/Worker participants: they use their selected original workspace directly unless an explicit cross-Host Transfer requires a Pastey task workspace.

## Provider and runtime configuration boundaries

Provider configuration is Host-owned. Non-secret endpoint/model/timeout/token-limit metadata and a generation/config digest live in SQLite. The credential is stored in a separate authenticated-encrypted row using the existing Host master key and is materialized only into an immutable process-local binding.

One managed attempt stores an exact provider id, generation, config digest, and model. Updating configuration creates a new generation and cannot silently alter an active run. Stale references fail closed. Deletion revokes live bindings and interrupts bound attempts; active OpenAI-compatible I/O observes cancellation/revocation and drops its request promptly. There is no automatic fallback or authority widening. Provider health is a bounded reachability/authentication/configured-model/response-shape observation only, not authority.

Credentials never enter prompts, Worker history, observations, effect requests/evidence, status events, or normal DTOs. The model cannot select endpoint/model/configuration, and switching provider cannot change the `StepWorkDescriptor` or effect envelope. Environment-variable configuration exists only in an ignored development smoke path.

The Task Provider screen is a local Host configuration client, not a provider or Worker authority surface. It can create, edit, delete, explicitly select, and run the bounded no-effect health probe for the existing OpenAI-compatible configuration. Its snapshot contains only provider id/generation/config digest, endpoint, model, timeout/token bounds, health/timestamps, and selected/stale state; it never returns a credential, encrypted row, resolved binding, revocation token, Worker handle, or provider response. Saving a new key sends it directly to the local Host command and clears the password input; editing without one preserves the encrypted key. The runtime section still exposes only known logical identities and availability/selection/readiness; selection asks the local Host service to discover and pin the executable privately. Once a selected runtime exists, production readiness creates the exact Execute process binding rather than requiring a test/acceptance caller to inject it.

## Natural-v2 and PM

Natural-v1 remains intact for the v1 compatibility path. Natural-v2 reuses its useful constraints—bounded structured output, strict validation, risk scanning, derived review text, and provider output as proposal-only—without inheriting Search-first, `selected_file`, two-device, or implicit two-party assumptions.

`CandidateSemanticPlanV2` names only Core-provided aliases for Hosts, roots, routes, steps, and object flow. It supports generic roots, Search outputs, explicit dependencies, Transform, explicit Transfer, and Execute. A constrained local interpreter and an explicitly selected proposal provider receive the same bounded facts and produce the same schema.

Core then:

- rejects fabricated, stale, ambiguous, or unselected aliases;
- revalidates the requester and every Host against the current Bridge;
- revalidates exact local managed roots;
- derives Search object identity and Transform N+1;
- checks producer dependencies and exact location flow;
- rejects implicit Host switching or model-invented movement;
- calls the native-v2 Composer only to create an unapproved Draft.

The review DTO exposes bounded topology, movements, and affected Hosts for presentation. Capability facts remain observations and cannot authorize execution or movement.

## Native-v2 orchestration and completion

The requester stores the Draft, approval, attempt, per-Host readiness/admission, and per-step status. The barrier is transport-neutral: remote participants use Room Control and the requester participant uses direct local actions against the same receiver state machine. After readiness/prepared commit, each Host runs only the next locally authored dependency-eligible step:

- Search uses the existing bounded candidate/safe-file path and binds the exact declared output;
- Transform/Execute invoke one Worker run and finish only through the Core result path;
- Transfer uses the existing Layer 5 → Layer 3 → Layer 4 → Layer 1 encrypted transfer path.

A Host returns a bounded correlated result only after local authoritative completion. The requester verifies the exact participant/Host/session/revision/dependencies and commits it once. Remote participants receive the commit through Room Control; the requester participant receives the same transition directly. Only then can a participant consider that predecessor complete. When the requester is the Transfer destination, the exact attempt/step/revision/hash/object revision/content digest/destination receipt is validated inside the authoritative requester transaction before the shared step commit becomes visible; remote destinations enforce the same receipt gate when accepting the commit.

Bridge Device Check remains a fixed Generic Managed E2E self-check: explicit user intent creates one ordinary `Search @ requester → authored Transfer → Execute @ exact remote Host` revision and approval, then uses the same readiness, admission, Worker, EffectEnvelope, ExecutionWorld, and Core finalization. It is not the Native mature-Agent path or its roadmap gate. Missing selected provider, runtime, or ExecutionWorld facts are a preflight `BLOCKED`; its renderer-safe report mechanics are owned by [Layer 2](layer-2-device-intelligence.md).

Transform finalization seals either one OutputSlot file generation or, for `output_selector: "."`, the complete tracked regular-file-set, then registers N+1 at the same Host. Execute records only its result digest. Provider/model/Worker output is always non-authoritative.

## Lifecycle and recovery

User cancellation marks requester and receiver attempt/dispatch state terminal, cancels an active Worker/provider request, terminates process worlds and transfers, revokes Core run authority, and rejects late completion. Requester-local cancellation follows the direct local action path. Disconnect/session replacement, provider revocation, Burn, shutdown, and restart use the same fail-closed principle. Restart restores no process-local run, local session binding, grant, world, provider binding, or Worker session.

Successful Core completion cannot be converted back into cancellation during the small completion critical section, but a terminal global interruption rejects a later product result/continuation. Failed or cancelled steps never unlock dependencies. Duplicate and late remote completion is rejected by immutable/unique commit state.

Distributed delivery failure remains unable to prove remote native-process termination across a partition. The existing card reopens one unresolved durable fact at a time and loads the next after resolution. Remote uncertainty offers Reconcile and Stop; `native_agent_outcome_unknown` is observed again after reconciliation so the card can see the updated durable fact. Recovery controls use the durable task/movement projection and the card loads that correlation before rendering Reconcile or Stop. A durable result Return offers Retry result Return without rerunning the Agent or resending its workspace. After requesting a source apply retry, the mounted card polls the same movement projection for up to 20 seconds, stopping early when it observes the next durable resolution. `conflict_recovery_required` offers Reveal result and Discard result. Consequence-side interrupted recovery offers Abandon recovery, which cancels only the movement authority and does not rewrite a Completed Agent task. A lost final outbound Transfer acknowledgement, or a sender-local item-status write failure after receiver `/finish` success, is treated as execution-ambiguous and retains source ownership. If workspace preparation was accepted but outbound Transfer still provably never finalized, the source records ordinary interruption and makes a best-effort authenticated cancellation of the receiver's queued preparation; cleanup delivery failure does not fabricate execution ambiguity. Transient route/session loss invalidates Bridge transport authority without interrupting a live Host-local native observer. A completed task with a durable exact snapshot remains retryable across restart; when a crash precedes snapshot capture, restart seals the result from the retained app-owned task workspace and makes only that Return retryable. Burn performs destructive Bridge-scoped cleanup. An unprovable apply journal leaves its canonical and hidden sibling trees untouched while the burned Bridge's envelope is removed; that ambiguity cannot make burned-room startup finalization permanently fatal. Native observer persistence updates an existing envelope only and cannot recreate one deleted by Burn. A received workspace is durably bound to its movement before the Agent starts; a later movement projection write failure cannot cause landing cleanup to delete a running Agent workspace.

## Current capability matrix

| Can today | Intentionally unavailable or incomplete |
| --- | --- |
| Host-native Codex capability discovery/invocation and Host-private native sessions | Generic capability acquisition/install behavior; Agent integration does not authorize installation |
| Read-only, local discovery and inspection of existing Claude Code and Codex sessions in Host-named workspaces, with live state from provider sources; remote read-only discovery and inspection from a trusted device the Host shares a folder with (Sessions on), with minimal disclosure; Codex runs in a folder the Host shares, named by reference; local continuation of an unheld Claude Code session through `claude --bg --resume`; continuation of an existing Codex thread under Codex's writer lock, locally and, after the Host's Review, from a device that may read its folder | Claude Code continuation from a Bridge peer; attaching to or steering a running turn of the Codex app; Mac ↔ Windows physical validation of remote reading and continuation |
| Capability-neutral native lifecycle with adapter dispatch by capability ID, opaque bounded input/output, and generic invoke-v2 (exercised by a test-only adapter) | Product UI for invoke-v2 or for any non-Codex native capability |
| Native Agent local original-workspace execution and direct remote invocation for a workspace already on the selected Host | Automatic or invisible topology mutation; remote movement remains visible and consented |
| Native executor admission: immutable Local/Bridge task ownership, Bridge-bound cancel, and a one-shot, process-local Accept/Deny executor Review for every remote invocation and received workspace, unless the executing Host's user gave the paired requesting device Full Access | Per-capability ACLs or per-folder allowlists; physical Mac ↔ Windows evidence of unattended Full Access |
| Required remote workspace movement detection, requester movement Review/approval plus executor Accept, encrypted outbound/return movement, restart recovery, source revalidation, and durable conflict recovery | Physical Mac ↔ Windows movement and failure-matrix evidence |
| Bridge-native NodeList/capability projection, bounded fixed Host probes, and generic semantic-ID capability-acquisition confirmation | Confirmation performs no installation or execution |
| Deterministic native-v2 Draft/Review/approval/readiness/status/cancel backend and 2.0 lifecycle UI for an opened revision | Renderer-safe Draft discovery/origination, PM context, reviewed topology, and result projection |
| Proposal-only local/provider Natural-v2 to an unapproved Draft | PM/provider selection and settings presentation |
| Whole-Plan remote and local-Host readiness, prepare, attempt-bound admission, commit, and exact continuation | Headless Host execution |
| Remote or local-Host Search and authored encrypted Transfer with exact receipt | Automatic topology repair for Generic Managed Plans |
| Same-Host Resource Worker Transform; Host-selected runtime settings/resolution and exact Execute binding; local provider configuration/selection/health UI; contained Process on verified macOS; native Windows Managed Execute acceptance through the Codex-backed production path | Linux process world |
| Execute through Core with no lineage when an exact process binding exists | Raw shell/terminal/process authority |
| Durable generation-bound provider state, streaming adapter, and safe local configuration/health product surface | Provider marketplace, routing, fallback, or additional provider backends |
| Bridge Device Check fixed Generic Managed E2E self-check through the ordinary native-v2 lifecycle | Its external-provider/API and packaged Mac ↔ Windows evidence remain Generic Managed subsystem limits |
| Phase 5 Host network broker | Worker network tools or automatic task egress |
| Cancellation/revocation/restart/Burn fail closed in state and Core authority; Bridge Burn purges only Bridge-bound Native Agent outer state and retained app-owned results | Guaranteed cross-partition native-process termination |
| Bounded non-secret product/Worker status events and authoritative lifecycle presentation, including Native Agent reconciliation/Stop/Return repair | Result content projection or Native Agent history/task browsing |

Subagents, Headless Host, Worker network, Developer Terminal conversion, task-specific patch/document engines, and task/command allowlists are absent from the Generic Managed Worker subsystem. Native Agent source-level reliability and deterministic two-Host state validation are complete; they do not prove physical Mac ↔ Windows behavior. Physical validation remains Phase 2 work. The test-only harness and its limits are described in [development](../development.md).
