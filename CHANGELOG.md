# Changelog

Detailed update and release history for Pastey.

The current development changes belong in `Unreleased`. Each release archives that section under its version while leaving a clean `Unreleased` section for later changes.

## Unreleased

### Changed

- **Full Access replaces unattended Codex permissions.** On the device to be controlled (say a Windows PC at home), Bridge details → **Full Access** → Allow Full Access… lets the device that Bridge is paired with (say a MacBook elsewhere) control it through Pastey without anyone there clicking Accept: run Codex in any folder, continue Codex sessions, read the sessions in its shared folders, add folders by path, send a folder to run on, and open a Developer Terminal.
  - Informed and explicit: the dialog says what it allows (including that the terminal can do anything your user account can), what it never does, and what ends it, shows the paired device's fingerprint, and needs "I understand" ticked. After pairing, the Bridge shows a one-time notice that Full Access exists and is off.
  - Directional: it lets that device control this one, never the other way round. Each device gives it only for itself, on itself.
  - It lasts through reconnects and restarts. Revoke (Bridge details or Settings → Permissions), revoking or forgetting the device, pairing the Bridge with another device, and Burn each end it; trusting or pairing the same device again does not bring it back.
  - It applies only to requests over a session where the paired device proved its identity key. Pastey never answers Codex's own questions (when Codex's settings make it ask, the question goes to the requesting device), never continues a Claude Code session from another device, never answers UAC, administrator or macOS privacy prompts, and leaves Plan approval unchanged.
  - The controlled device shows a terminal the other device has open on it, with **Close terminal**. Revoking refuses new requests at once, closes those terminals and asks the Codex tasks Full Access started to stop; the requesting device shows "Stop requested: Full Access ended" (or "Stop requested · not confirmed" when the stop could not be delivered), never "Stopped".
  - Everything Full Access starts has exactly the rights of the Pastey process: normally your account's, never more. If Pastey runs as administrator, the consent dialog says that the other device would get administrator rights too. Folder rules only decide which folders are offered for sessions; they do not limit the terminal or Codex.
  - On the controlling device, Agents shows "Full Access" for that device and **Workspaces on <device>** with an Add a folder… button. The Access dialog says whether that device gave this one Full Access.
  - Existing unattended permissions are removed on update, not converted: those requests wait for Accept until Full Access is given. Trusting a device still allows nothing by itself.

- Agents is organised as the work is: **Agents on [device]**, the **workspaces** it shares with this Bridge (each with what it allows, Sessions and Agents), and, in a workspace, **New conversation** and its **sessions**. One conversation is open beside the navigator, or alone with a back button at the default window size.
  - A session opens as a conversation: its prompts and replies (this device's sessions: the newest 40 messages; another device's: the short excerpts its Host shares, if any), then your turns in it, then a composer that continues that session. State, who has it open, ids and evidence are under Details.
  - Each turn shows where it is (waiting for Accept, running, an approval it asks for) and then the reply, inside the conversation; failures say whether anything ran and what to do, with Reconcile, Stop and the other repair actions in the turn. Pastey does not show a reply before it has one: Codex's partial output is never read.
  - A **New conversation always starts a new Codex conversation**; it no longer continues Pastey's earlier conversation in that folder by default, and the "Start a new conversation" checkbox is gone. Replying in the same view continues that conversation; Pastey's earlier conversations are listed as sessions and continued from there.
  - The followed task is shown once, in its conversation; a banner in the navigator leads back to it while it runs or needs action.

- Agents is reorganized around what you came to do. The top says which device the agents work on (and that it decides what this device may do there); below it, the one task this Bridge follows; then one choice: **New task** or **Continue a session**, one view at a time.
  - Continue a session lists the sessions by itself once there is a folder to look in, each with whether it can be continued from here. A session's view starts with that answer and what happens if you continue, then its state in plain words and who has it open; facts, unknowns, live sources and ids are one step away under Evidence and how Pastey knows.
  - Continue session is followed where you started it: the form is replaced by the run's progress (Sent → Accepted there → Running → Result), and when it ends the session is read again, showing the new turn.
  - Every outcome says whether anything ran and what to do next: "Did not start · nothing ran" with the other device's reason and the fix (for example, close the thread in the Codex app there), "Declined on Windows · nothing ran", "Nobody accepted in time", "Codex reported the turn failed" (it did run; the session now ends with that turn), or "Outcome unknown" with Reconcile. A finished task can be dismissed.
  - New task says that it continues the conversation Pastey keeps in that folder (listed as "Started by Pastey"), and points to Continue a session for conversations from the Codex app or a terminal.
- Who has a session open is said with how Pastey knows it: Codex's writer lock for that thread (macOS), the device's list of running Codex programs (Windows, where the lock cannot be read; it covers every client at once), or Pastey itself. When Pastey cannot tell, the view says "You can try: Codex decides when it starts" instead of "You can continue".
- A thread that only Pastey's own idle conversation has open is reported as such, not as "open in another Codex client", and can be continued: Pastey closes its own idle session of that thread first (nothing is lost) and Codex's writer lock still decides. A thread a Pastey task is running in is reported as running and is not continued. On Windows, Pastey's own session no longer makes every other thread in that folder "Unknown".
- The Accept request on the executing device names its own shared folder the request is for.
- Continuing a Codex thread now checks, before the thread is taken or anything of Pastey's is closed, that Codex records it for exactly the folder admitted (a peer's thread id is only its claim); Pastey's own idle session is closed only for that same folder. After closing it, Pastey waits for the lock itself on macOS rather than the lock file, which a killed app-server leaves behind, so the continuation no longer stalls for 5 s. A run lets go of its app-server and lock before its folder is free for the next task.
- Continuing a Codex thread requires the Codex that Pastey launches to report a version that keeps thread writer locks (0.163.0-alpha.10.2 or later); otherwise nothing runs. Before, a lock file left by a newer client (or held by one) could pass the check for an older Codex, which would then write alongside that client.
- A request to another device whose connection ended before it was sent now ends as "Not delivered · nothing ran" and stays shown, instead of failing with an error and leaving a queued task nobody answers.
- On Windows and Linux a missing Codex lock file is no longer reported as "not open in any Codex client": where Codex keeps its lock there is not measured, so the device's process list decides what is shown. Starting a continuation never relied on it.

### Added

- **Bridges across the internet** (Settings → General → Across the internet, off by default). Devices already paired on the same network can reach each other from different networks: directly through NATs when the networks allow it, otherwise through a relay. Both devices must turn it on. It uses iroh (QUIC); the relay is n0's public relays (for development and hobby use) or one you run.
  - The Bridge itself is unchanged: the same pairing, signed reconnect, encrypted Room Control, Reviews and permissions. The internet path only carries a Bridge's encrypted traffic to a device key learned over that Bridge's authenticated session, and it is revoked with the Bridge (Burn, departure, forgetting the device, pairing another).
  - Bridge details and Device details → Connection say whether the internet path is in use, and whether directly or through a relay. A relay is slow for large files (about 1 MiB/s measured on n0's free relays).
  - A network change or a dropped path never reruns or resends Agent work; an uncertain outcome is reconciled as before, and nothing waits to be sent later.
  - Not yet validated across separate physical networks; part W of the Mac ↔ Windows acceptance is the procedure.

- Room Control rate budgets per kind of message: capability checks, their answers, Agent requests and statuses, Bridge Plans and membership each have their own, so a polling UI cannot use up what a running task needs. A refusal for the budget is retried automatically, with a fresh message, for a few seconds where it is safe (it proves nothing was done).
- A request the other device refused before acting on it (its rate budget, its limit of 8 requests waiting for Accept, or this device's authority) now ends as "not delivered · nothing ran" with that device's reason, instead of an unknown outcome to reconcile.
- Device Details → Advanced → Control channel: what this device accepted and refused over the Bridge, by kind and reason, current backpressure, what it could not deliver, and the requests waiting for Accept. Counts only.
- Share a workspace with a Bridge from inside it: Bridge details → **Workspaces** → Share a workspace… opens this device's own folder picker, and each shared folder has its own switches for **Sessions** (with or without short excerpts) and **Agent requests**, an expiry, Allow without asking… (a few unattended Codex runs in that folder), and Stop sharing. It is for the Bridge's own device; if that device is not trusted yet, Trust… with the fingerprint is offered in the same place, once. Sharing a folder allows nothing until a switch is on, and trusting a device shares nothing.
  - The other device sees the folder only by its name and a reference that differs per Bridge, never its path. The same section shows what the other device shares with this one, and exactly where to change it there.
  - Agents uses those shares: Run a task offers **Shared by <device>** (choose the folder by name; that device finds its own folder), Sessions on <device> lists sessions only of folders shared with Sessions on, and continuing a Codex thread there needs both switches. Each run or continuation still waits for that device's Accept (runs only: unless you allowed some without asking). Typing a path on that device, and sending a folder from this device, are unchanged.
  - Every missing permission is explained where it is needed, with the exact step on the other device, for example "Windows hasn't shared any session-readable workspace with this Bridge. On Windows, open this Bridge, then Bridge details → Workspaces and choose Share a workspace."
  - While a Bridge is open, Pastey shows this device's identity to its other devices once per connection (it grants nothing), and before each session query, so a reconnect or a restart on the other device needs no extra step.
- Continue an existing Codex thread or Claude Code session in the session itself, from Agents → Sessions → open a session → Continue session. The run keeps the session's id and history and appears under Run a task with Stop, its approval prompts and its reply.
  - Codex threads use Codex 0.163's own thread writer lock: Pastey's dedicated app-server takes it with `thread/resume`, so Codex refuses while the Codex app or a terminal has the thread open, and closes it after the turn. Pastey also checks, under the lock, that the thread still ends with the turn you saw, belongs to its folder, and that the Codex it launched keeps writer locks.
  - Codex threads on another device can be continued too: the request names the folder only through your Session reading permission there, and every continuation waits for that device's Accept.
  - Claude Code sessions are continued on this device only, as before through `claude --bg --resume`, now also only while the session still ends with the entry you saw.
  - Sessions report a "last entry" marker for that check, and Codex discovery reads each thread's writer lock: "open in another Codex client" or not, even while the Codex app runs. Sessions Pastey started are labelled.

- Paired devices connect without a code. After two devices pair once with a code, each remembers the other's identity key.
  - Nearby shows such a device as "Paired before", and **Connect** starts a new Bridge with it, even when it already has one, without a code or an Accept prompt on the other device. The other device shows a notice.
  - The connection still proves both identity keys. A device that only claims to be paired, or that was forgotten, gets the ordinary prompt-and-code flow. Other devices on the same network never connect without a code.
  - Settings → Paired devices lists the remembered devices. Forgetting one makes it need a code again and ends the current session of each Bridge with it.
- Unattended Codex access: on the device that runs Codex, its user can trust a specific requesting device (after comparing identity fingerprints shown on both) and give it a limited permission to start Codex in one folder over one Bridge without clicking Accept, for up to 7 days and up to 50 runs, revocable at any time. Devices prove a durable identity key bound to the exact current Bridge session; addresses, names and codes never count as identity. Without a valid permission, requests still wait for Accept. Codex's own approval requests are never answered by a permission.
- Bridges can form and reconnect across networks through a user-managed overlay such as Tailscale or WireGuard: join by code at the other device's private or overlay address, or remember that address for an existing Bridge. Discovery is unicast to that address only; pairing and reconnect are authenticated exactly as on the LAN, and an address never authorizes anything. Public internet addresses are refused, including a name that resolves to one (every reconnect resolves the name again) and numeric forms such as `0x08080808`. Join by code has an "another network" option, and the Bridge header shows the connection state and lets you set or remove the remote address, with errors that say whether the address, the code, the Bridge or the network is the problem.
- Find file: from a Bridge, search another device for a file by name (for example a validation `.mp4`) and receive only the one you choose in this device's Inbox, through the existing reviewed Search → Select → encrypted Transfer flow. Nothing else moves and nothing on the other device changes. Results show name, size, modification time and a redacted location, and expire after 10 minutes.
- Shared search folders: under Settings, a device's user can share specific folders (for example a project's output folder) for Find file, and stop sharing them. Other devices can only name "shared search folders", never a path. Folders too broad or private to share safely are refused. No folder, including Downloads, Desktop, Documents and Pastey Shared, is searchable by another device until shared here for its Bridge.
- A Codex conversation started through Pastey now survives a Pastey restart: a follow-up in the same workspace from the same requester reopens the same native conversation with Codex's `thread/resume`. The task card says whether a task continued the conversation or began a new one, and offers "Start a new conversation". If Codex cannot reopen the conversation, nothing runs and the card says so; Pastey never silently starts a new one. Conversations are bound to their requester (this device, or one peer device over one Bridge) and are forgotten when that Bridge is burned.
- Native capability invocation is now capability-neutral. Core dispatches each invocation by capability ID to a registered adapter that owns its input, start, stop, and terminal output; Core keeps task identity, ownership, admission, status, cancellation, reconciliation, restart recovery, Bridge authority, and the durable envelope. Codex is the only shipped adapter. A capability-neutral `pastey-native-agent-invoke-v2` invocation and its `start_remote_native_capability_task` command exist without a renderer entry point yet.
- Native capability input and terminal output are opaque, capability-owned payloads bounded to 16 KiB. Core carries, persists, compares, and digests them without interpreting them, and Room Control's field-name safety check skips exactly those payload subtrees while still checking every other field.
- A completed Codex task now returns Codex's actual final reply, captured from the exact turn's completed agent messages and bounded to the output limit, instead of only a completion summary. It reaches the requesting device through the existing status and reconciliation path, and the task card shows it as escaped plain text, separate from the task state, including when it came from another Host.
- A Codex turn blocked on a native approval or question is shown as running and waiting for a native approval, on both Hosts, instead of plain running. Stop interrupts the turn.
- Codex command and file-change approvals of a task started from another device are shown on that device, with the exact command, folder and stated reason, or the exact file changes, as plain text. Its user can Approve once or Deny each one; the executing device checks that the decision comes from the same device that started the task, in the current session, for exactly what was shown, and gives Codex that one answer. Pastey never approves for a session, never answers on its own or because of an unattended permission, and never answers permission requests, questions or forms, which still wait until stopped. Requests that cannot be shown completely and safely cannot be approved.
- Selecting a remote device in the Codex task card now shows that device's own advertised Codex availability, with Check again, instead of this device's.
- See the Claude Code and Codex sessions on another of your devices, from the new "Agent sessions" card in a Bridge: choose the device, filter by agent, text or recent activity, and open one session for a report.
  - The report shows its status and how it is known, how its last turn ended, what it did, the folder's Git state, and what is not known.
  - The other device must allow it first, under Settings → Session reading: one trusted device, one folder, one Bridge, for up to 30 days, revocable. Trusting a device does not allow it.
  - It never sends folder paths or whole conversations. Short excerpts are sent only if allowed.
  - Reading never starts or changes a session.
  - Verified between two separate device states in automated tests. Not yet run between a real Mac and a real Windows PC: see the acceptance procedure.
- A discovered Claude Code session can be continued with a new prompt, through a local command for now. Pastey uses Claude Code's own background resume, which continues the session only when no other Claude Code process has it open (otherwise Claude Code makes a copy, which Pastey stops and reports as an unknown outcome).
  - Pastey also checks Claude Code's active sessions right before starting and refuses while any process holds the session.
  - The task shows running, waiting for an approval that Claude Code's own window must answer, completed with the agent's reply, stopped, or unknown. Nothing is sent twice.
  - When the turn ends, Pastey releases the session so the desktop app or terminal can use it again.
  - Other devices cannot use it.
  - Codex threads found this way are not continued: Codex cannot show that its desktop app does not have a thread open.
  - Verified on a Mac with Claude Code 2.1.292.
- Read-only discovery of existing Claude Code and Codex sessions on this device, through two local commands for now (no screen yet).
  - Pastey lists the sessions in the workspaces you name: Claude Code from the desktop app or the CLI, and Codex threads from either.
  - Each session shows its workspace, origin and last activity, and whether it runs now, from Claude Code's own active-session list or, for Codex, only when no Codex process could hold it. Otherwise it says unknown.
  - It also says how the last recorded turn ended and whether resuming would be safe.
  - A report on one session gives its recent prompts and replies (labelled as the agent's own claims), activity counts and Git facts about the workspace, each marked as fact, inference or unknown.
  - When several sessions match, all are returned for you to choose.
  - Nothing is started, resumed or changed. Other devices cannot use it.
  - Verified on a Mac with real sessions. Windows is cross-compiled only.
- Codex works with only the ChatGPT desktop app installed. Pastey finds the Codex that the app bundles, through the app's own layout manifest, and starts it as its own app-server, with the user's existing Codex sign-in and configuration. No separate CLI install and no API key are needed.
  - A Codex CLI on PATH is still preferred when it works; otherwise the first working desktop app is used.
  - The task card says which one is in use.
  - Pastey's sessions are separate from the app's own: they appear in its history, but approvals stay in Pastey.
  - Verified end to end on a Mac with the desktop app, with a real task. On Windows, desktop discovery is unit-tested and cross-compiled only; `scripts/diagnose-windows-codex.ps1` now also reports a bundled desktop Codex.

### Changed

- Session reading permissions are now workspace shares. Existing ones keep allowing session reading in their folder, and allow no agent requests until you turn that on in the Bridge. Continuing a Codex thread on another device now needs that device's share of the folder to allow agent requests as well as sessions (it needed only reading before), and the Mac's continuation request is refused otherwise before anything is queued.
- Settings → Permissions → Session reading became **Shared workspaces**: every folder this device shares, across Bridges, with Stop sharing. Sharing and changing a folder happen in its Bridge, so the separate "Add a permission…" form, which made you choose the device and the folder again, is gone (as are its `create_session_read_grant` and `revoke_session_read_grant` commands).
- Stopping a share also ends any unattended Codex permission that device holds for that folder on that Bridge.
- The Agents tab's Access summary is built from what the other device says it shares, and its steps point into that Bridge on the other device instead of its global Settings.
- The desktop UI is reorganized around Bridges, with one design system (type, colour, spacing and components) for light and dark themes. See [UX and information architecture](docs/ux-information-architecture.md).
  - The sidebar holds only device-wide places (New Bridge, Inbox, Settings) and the Bridges. Each Bridge has two tabs: **Share** (text and files, both ways) and **Agents**. Its connection, pairing, devices, what it may do on this device (with Revoke) and Burn are in **Bridge details**.
  - **Agents** is one workspace per device: choose the device once, then **Run a task** (Execution) and **Sessions on <device>** (read only) appear as separately labelled sections. "Access on <device>" lists each authority apart (pairing, running Codex, Codex's own prompts, reading sessions), what is known here, what the other device decides, and the exact steps there. Sessions Pastey started are marked "Started by Pastey".
  - **Device Details** (click a Bridge's device name, a device in Bridge details, or Device details in Agents) gathers one device's overview, connection, measured performance, agents, trust in both directions, and the managed self-check.
  - **Find file** is now **+ → Get a file from <device>…** in the Share composer, in a dialog. The flow and its review are unchanged.
  - Settings has seven sections: General, This device, Devices & trust, Permissions, Agents, Advanced and About. This device holds the identity, the device profile, the runtime check and the local speed test. Permissions lists every grant (unattended Codex, session reading, shared search folders) with Revoke first and the add form one step away.
  - Settings no longer shows "Burn after download", which the Host never reads, or rows that only said "Unavailable". The stored value is kept.
  - The default 760 × 760 window no longer squeezes the Bridge between two side panels; narrow windows get a collapsible sidebar.
  - Files dropped on the window are sent from the Share view, as its placeholder always said.

- A remote native capability invocation now requires the executing Host's executor Review before anything runs. Authentication and capability availability no longer suffice: the invocation waits, unprepared and unstarted, until the Host user accepts it under the exact session it arrived on; Deny, expiry, session loss and Burn end it without starting. Local invocations are admitted immediately by the same policy. Executor Reviews are process-local and show no invocation input.
- An approved workspace movement no longer runs Codex on the executing Host by itself. After the workspace lands, the executor shows the same Agent request as a direct invocation; only its Accept starts Codex, and Deny, expiry, session loss, Burn or restart end the movement as a definite non-start and remove the landed workspace. Requester movement approval is unchanged and still required; it authorizes the movement, not execution.
- The requester's workspace-movement Review no longer says Codex will run: it says the receiving device runs Codex only if it accepts, and its button reads "Approve and send".

### Fixed

- Test connection could report a peak far above anything the link carried (0.2 MB/s average, 14.1 MB/s peak, over a phone hotspot): chunks that shared a slow link and finished together were counted as a burst. The peak now spreads each chunk over the time it took, so it is never below the average and shows only rates the chunks show. The average, duration and quality were right and are unchanged. Each Bridge test also logs its chunk timing and the internet path it ran on (direct or relayed, round-trip time, losses, congestion), so a slow uplink can be told from a stall. The test also no longer says it takes about a second: it waits for what it sent.
- "Current remote Host session is unavailable" could appear while the Bridge showed Connected: the Bridge keeps a session through a few lost liveness probes, but a query gave up after one. A query now tries the same proof once more. Codex availability and what the other device shares (including its Full Access) are asked again when its session changes and 5 seconds after a failed check, instead of waiting up to 15 seconds or for the window to be refocused.
- With Full Access given, a conversation still said the other device must accept each request. It now says the run starts there at once (or that it waits for an Accept, when that device said it gave no Full Access), and a task's progress says "Waiting for Accept" only once that device says it waits.
- Developer Mode could refuse with "Developer Mode UI authority is unavailable" until Pastey restarted: its UI session lasts 30 minutes and also ends when the Bridge reconnects, but the window kept offering requests with it. Developer Mode now says it ended and offers Enter Developer Mode again; Accept and Deny on the controlled device enter again by themselves. A terminal already open keeps working until its own end, and is never reopened or extended.
- Room Control stopped accepting anything after about 64 Codex availability checks ("Room control inbox is full"), so Run with Codex and Continue session failed until Pastey restarted. Every capability check and its answer was kept in a per-Bridge inbox that nothing emptied. Capability, Native Agent and Bridge Plan v2 events are now acted on and not kept; only Bridge Plan v1 events are kept, as bounded display history (64, 30 minutes, oldest dropped first) that never refuses anything. Reproduced and covered by a test that runs 500 real availability checks between two Hosts over the real Room Control handler (it failed at check 65 before).
- Capability and Bridge Plan events were answered, recorded or started before their replay and rate checks, so a replayed or refused event could still have acted. Admission (replay window, then rate budget) now comes first for every kind; a refusal acts on nothing.
- Answering a capability check could run Codex's availability probes inside the request handler when no observation was cached yet (first checks after start), blocking other requests long enough for the 1.8 s liveness check to fail ("Current remote Host session is unavailable"). The answer is now built after the receipt, on the blocking pool, and concurrent first probes share one.
- A task's final status (for example Completed) was sent once, and if that delivery failed the Mac learned the outcome only through reconciliation. It is now resent until delivered or its bounded resends are used up, and a status refused for the Mac's rate budget is offered again for up to two minutes without using them up.
- Accepting a Review did not check again that the folder was still shared. Stopping the share, turning off Agent requests (or, for a continuation, Sessions), its expiry, revoking the device or Burn while the Review waited now make Accept start nothing.

- Revoking a device's trust left its session reading permissions unrevoked, so trusting the same device again later silently brought them back. Revoking a device now revokes its workspace shares too.
- After a reconnect or a restart of the other device, reading its sessions said "not allowed" until something else made this device show its identity again. Session queries now show it first.
- Restored what earlier UI rewrites had dropped although the Host still supports it: changing the receiving folder (and going back to the default), Save received files and Save received images (the Host uses them to choose where a received item lands; an earlier version of this entry wrongly said it does not), editable transfer grouping and transfer window, the device profile and runtime check, the local speed test, and a read-only connection test with a measured throughput for each Bridge device. The 2.0 frontend (August) had already replaced the speed test and device profile with placeholder rows.
- Codex's stderr was read only up to 64 KiB and then left unread, so a talkative Codex filled the pipe and blocked mid-turn; the run never ended and could only be reported as unknown. Pastey now drains it to the end.
- After switching Bridges or reloading, a task that was simply running disappeared from the card, and with it Stop and its approval prompts: recovery lists only tasks that need resolution. The card now follows the last task each Bridge started by its exact id.
- The indeterminate notice now says what happened, what Reconcile can tell, to check the folder on the other device, and that Stop undoes nothing and nothing is re-run.

- Run with Codex could stay disabled with no explanation. For a run on another device the button needs that device's folder path typed on this device, and the field was easy to miss (choosing a folder in the other device's Session reading or Unattended access settings does not fill it). The card now lists each requirement, says why the button is disabled, separates a folder already on that device from one sent from this device, and never depends on Session reading.
- A Codex target chosen on another device fell back to this device when the Bridge reconnected with a new session. The target is now kept by its Host identity and shows "not connected" until it returns.
- Another device's Codex availability was checked only when it was selected or on Check again. It is now checked again every 15 seconds and when the window regains focus, keeping the last answer visible meanwhile.
- Right after Run with Codex on another device, the task card forgot which device the task ran on: recovery lists only tasks that need resolution, so its empty answer cleared the device. The card named no device and treated the task as local, so Codex's requests on it offered no Approve once or Deny here. The started task now stays bound to the device it was sent to, and Stop and decisions go there.

- Windows: Codex installed as an app execution alias (in `%LOCALAPPDATA%\Microsoft\WindowsApps`), or installed after Pastey started, was reported as unavailable. Pastey now:
  - finds aliases;
  - also searches the current user and system PATH;
  - starts Codex with that PATH, so an npm-installed Codex finds Node.

  When Codex still cannot be used, the task card says why: not installed or not found, could not start, exits with an error (with its message, on the device itself), did not answer in time, or incompatible. It also says what to do. The requesting device sees the same reason. `scripts/diagnose-windows-codex.ps1` collects read-only facts for diagnosis. This is unit-tested and cross-compiled; it is not yet validated on a Windows machine.
- Security: joining a Bridge is now authenticated end to end. Previously anyone who could reach a device's Bridge server and learn the Bridge id, which the code lookup broadcast on the LAN revealed, could replace the connected device, send it text and files, and receive what the user sent next. Now:
  - The 8-digit code is a one-time password of a SPAKE2 exchange. Nothing derived from it crosses the network, so it cannot be recovered by listening or offline guessing. Each code answers at most 20 guesses, expires after 15 minutes, and admits one device.
  - Pairing binds the Bridge, both devices' identity keys and HostRefs, and both session keys. Each device remembers the other's identity key.
  - A Bridge reconnects only to the device it was paired with, which must prove its identity key over fresh nonces from both sides. A different key or HostRef is never accepted on reconnect.
  - Forged answers on the LAN are ignored.
  - Items and file transfers are accepted only from the connected peer. Transfer finish and cancel, liveness and benchmarks require a signed, single-use request from the peer, and liveness now proves the peer holds its session key.
  - Nearby join no longer sends the code: the accepting device shows a fresh code and the requesting device's user types it. Join prompts are limited to one per address and four in total.
  - Both devices must run this version: earlier versions cannot pair with or reconnect to it.
- Bridges from before this change have no verified device. They stay disconnected, with their local content kept, until paired again from the Bridge's own network panel: one device chooses "Show new code" and the other enters it under "Pair with code". A code entered under New Bridge never takes over a Bridge this device already has.
  - Pairing a device other than the one a Bridge last verified starts fresh: folders shared with that Bridge and its earlier Agent tasks, conversations and retained results are removed. Items and Plans stay.
  - The code is what decides which device is trusted, so give it only to your own device. Pastey does not treat a device's HostRef as proof that it is the original device.
- Security: file transfers are now bound end to end. Each chunk's encryption is tied to its transfer and position, so chunks cannot be reordered or swapped in transit, and the receiver checks the sender's signed BLAKE3 of the whole file before saving it. A malformed or forged chunk request from anyone else is refused on its own and no longer cancels the transfer.
- Security: another Bridge device could search this device's Downloads, Desktop, Documents and Pastey Shared, and receive a file from them, without this device's user sharing anything. The searched device now refuses those places for every search another device requested; only folders its user shared with that Bridge are searched. This device's own Plans (such as the Bridge Device Check) still search its built-in places.
- Security: after the last check of a file chosen through Find file (or another Bridge Plan Transfer), Transfer reopened it by path and followed links, so a process able to write in the shared folder could swap it for a link and have a file outside the folder read and sent. The chosen file is now opened once without following links, inside its approved folder, verified as the exact file selected, and read only from that handle; a change while it is sent fails the Transfer before it finishes.
- A single slow liveness probe no longer ends a Bridge session: three consecutive failures are required, and Room Control and capability-query waits now allow for internet latency.
- A shared search folder is now shared with one chosen Bridge only, instead of with every Bridge on the device. Folders shared before this change are not searched until shared again, and burning the Bridge ends the share.
- Find file now returns the 20 most recently modified matches, newest first, instead of the first 20 found, so a just-rendered video deep in a folder is no longer missed.
- A Bridge search result is no longer sent if the file was replaced with another file of the same size and modification time after it was found.
- Cancelling a remote Codex task that was in flight when the executing device restarted now releases its workspace instead of leaving it blocked.
- Starting a Codex task no longer holds the native service lock while Codex launches and opens its session, so status, cancellation, and remote status updates stay responsive; Codex availability is cached briefly and rechecked before each new session.
- A long or malformed Codex app-server line no longer ends task observation or releases the workspace early.
- On Windows, an npm-installed Codex (`codex.cmd`) is now found and started, and its app-server stops when Pastey closes the session (compiled and unit-tested; not yet validated on a physical Windows machine).
- A remote Codex task's card no longer disappears as soon as the task completes.
- Bound every Native Agent task to one immutable owner, Local or its Bridge. Another Bridge can no longer adopt, re-home, or detach a task through a duplicate or conflicting invocation, workspace preparation, or approval, and Burn again selects exactly the tasks its Bridge owns, including after restart.
- Executor-side `native_agent.cancel` now requires the authenticated Bridge to own the task; a Bridge cannot cancel a Local task or another Bridge's task.
- `native_agent.workspace_prepare` is accepted only when its source Host is the authenticated current-session peer `HostRef`.
- Cancelling or releasing a Codex task now acts only on that task's own app-server, never on a later session that replaced it at the same workspace.
- Native Agent shutdown no longer shuts down the Codex adapter twice.
- An unchanged Bridge no longer rewrites its legacy peer row on every Room Control event.

### Documentation

- [Mac–Windows acceptance](docs/platform/mac-windows-acceptance.md) part X retests the WAN findings of 27 October 2026 on real devices: the benchmark with its log fields, direct and relayed; the raw hotspot path; Codex availability through an outage; Full Access guidance; and Developer Mode after 30 minutes or a reconnect.

- Crash forensics: release builds abort on any panic and strip symbols, so a crash left no trace. Pastey now writes a bounded panic record (thread, source location, message) to its log before aborting, notes on the next start that the previous run ended uncleanly (also after an access violation or stack overflow, which run no panic hook), and logs each Native Agent step by task id. [Mac–Windows acceptance](docs/platform/mac-windows-acceptance.md) part K lists the Windows diagnosis steps, part L the device-detail checks.

- Added the Bridge join audit (`docs/layers/layer-4-join-audit.md`) with the findings, their fixes and the remaining risks. Layer 4 and the reference now describe pairing, pinned reconnect and authenticated room requests.
- Synchronized the README, architecture, Layer 4, Layer 5, reference, and development documentation with the capability-neutral adapter model, the opaque payload boundary, immutable task ownership and Bridge-bound authority, and two-sided Review: the requester's movement approval plus the executing Host's own Accept. The reference now lists the invoke-v2 schema and payload, `native_agent.retry_result_return`, the Review commands, admission codes and bounds, and the admission/adapter sources. The architecture now states its authority invariants explicitly, Layer 1 and Layer 2 cover landed workspaces and native capability facts, and the docs distinguish native executor admission from managed Host admission.

### Removed

- Removed the static product website (`site/`) from the repository.

## 2.0.0-beta.2 — Pastey 2.0 Beta 2 — 2026-09-25

### Fixed / Changed

- Made Unix resource-limit typing target-correct for Linux GNU without changing the enforced limits.
- Switched Windows release packaging to the production path that stages the pinned Codex sandbox sidecars and audits the bundle.
- Use NSIS for Windows SemVer prereleases because MSI cannot represent their prerelease identifiers; stable Windows releases retain NSIS and MSI.

## 2.0.0-beta.1 — Pastey 2.0 Beta 1 — 2026-09-24

Version 1.9.2 is the previous frozen Layer 1–5 packaged baseline. The implemented 2.0 architecture has completed source-level reliability and deterministic two-Host state validation. An initial unstable/beta 2.0 release may ship before physical Mac ↔ Windows Native Agent acceptance. The beta period gathers that evidence; physical acceptance is required before RC or stable readiness can be claimed.

### Added

- Added remote native Agent invocation over authenticated current-session Room Control. The invocation binds the exact selected Host, workspace, task identity, native capability, bounded lifecycle/status, and cancellation; an already-remote workspace requires no ManagedObject, Scratch, Worker, or Transfer.
- Added explicit cross-device native Agent workspace movement as one reviewed envelope: source-to-Agent encrypted movement, native work, and encrypted return movement are covered by one approval. Pastey captures the source baseline, validates the returned workspace, applies only to an unchanged source, and preserves a returned result for conflict recovery instead of overwriting a changed workspace. Frontend lifecycle and review controls expose the movement state.
- Added Host-native mature-Agent sessions. Mature Agents are Host capabilities rather than Managed Workers; the Agent retains its provider/authentication, model, native tools, sandbox, reasoning, and native conversation semantics. Pastey keeps Host-private persistent sessions, accepts only strict native terminal success, validates cross-device workspace representability before review, and retains conflict-recovery data durably.
- Added deterministic two-Host Native Agent validation with separate durable requester/executor states, a fake native Agent, and an external turn ledger. It exercises production movement and recovery paths, exact Return retry, conflict retention, apply idempotence, cancellation, Burn, and Agent-at-most-once without claiming physical cross-platform proof.
- Implemented Phase 5's generic managed authority substrate: domain-separated exact authority contexts and envelopes, monotone policy intersection, managed-run lifecycle, resource/process/network effect dimensions, deterministic tool lowering, replay/sequence/budget control, and ordered Host-authenticated evidence.
- Added Host-private managed revision, workspace overlay, output-slot, and scratch resolution over the existing managed-object and safe-identity boundary. Authoritative revisions remain immutable; only Core may accept sealed output as future lineage.
- Added the generic execution-world seam with a real macOS contained-process adapter and a Host-owned independently scoped TCP/DNS broker. Linux remains fail-closed unavailable; execution worlds retain no ambient/raw network authority. Windows was subsequently attached through the Codex-derived production backend described below.
- Added a one-use v2 managed-step claim and result-validation seam. Core alone may validate evidence and register exact same-Host Transform N+1 or an Execute result.
- Implemented the Phase 6 Worker Harness with bounded context/session state, provider-neutral streaming turns, alias-only Resource tools, optional exact Host-bound contained Process, structured observations, retry/compaction/cancellation, and Core-only result proposals.
- Added durable Host-owned provider configuration and exact generation/model selection with separate authenticated-encrypted credentials, bounded health metadata, immutable run bindings, and revocation.
- Attached authenticated inbound native-v2 managed attempts to a Host coordinator. It performs whole-Plan provider/Host/platform availability before admission, persists an immutable provider reference, atomically reserves one eligible local Transform/Execute, invokes one Worker run, and continues only after Core completion. Bounded lifecycle events and a Host cancellation seam are present; v1 is unchanged.
- Added deterministic native-v2 composition and requester orchestration with exact approval/revision/session correlation, complete-Plan readiness, two-stage remote prepare/commit, authored Search and Transfer execution, remote managed-step dispatch, exact result/commit propagation, bounded product status, and minimal compose/approve/start/status/cancel commands. An exact Transfer receipt is required before the destination accepts the same-revision commit; no implicit movement or Worker topology authority was added.
- Added proposal-only Natural-v2 local/provider interpretation with an alias-only candidate schema, strict validation and risk scanning, deterministic Core Host/object/revision/Transfer resolution, bounded topology review data, and lowering only to an unapproved native-v2 Draft.
- Added requester-local native-v2 execution through a domain-separated, process-session-bound self-binding and typed direct coordinator actions. Local Search/Transfer/managed steps now traverse the same Review, readiness, attempt-bound admission, prepared/commit, result, continuation, and cancellation lifecycle without Room Control self-routing.
- Added Host-owned managed runtime selection with exact safe executable identity pinning and production Execute-step binding. Capability observations remain separate and cannot populate executable authority.
- Added the Codex-derived Windows managed Process backend, Host-owned setup/verifier path, and native production Managed Execute acceptance. Physical Windows Stage 1–5 is complete with no unrestricted fallback.
- Added Bridge Device Check as the Generic Managed `Search @ requester → authored Transfer → Execute @ exact remote Host` E2E self-check, including bounded independent reports and verified fail-closed `BLOCKED / preflight_unavailable` behavior.
- Added Bridge-native NodeList projection and bounded current-session Host capability observations. NodeList remains environment facts only and cannot select a Host, change a Plan, insert movement, bind execution, or grant authority.
- Added bounded fixed Host capability probes with a global request vocabulary, Host-local implementations, and distinct `Available` / `Unavailable` / `Unsupported` / no-observation semantics. The probe path remains separate from exact execution binding.
- Added the low-friction Capability Acquisition confirmation foundation: exact `HostRef`, generic bounded semantic IDs, renderer-safe display facts, and only `confirmed` / `cancelled`. Generic capability acquisition/install behavior remains absent; confirmation performs no installation, probe, process binding, execution authority, Plan/topology mutation, Host selection, or Developer Mode change.

- Implemented Phase 4 as a parallel native `bridge-plan-v2` representation with Plan-scoped `HostRef` participants, generic managed-object roots, explicit per-step Host topology, exact logical revisions, dependencies, and a distinct `bridge-plan-revision-hash-v2` domain. V1 schema, hashes, protocol, UI, and Search/Transfer execution remain unchanged.
- Added Bridge Plan protocol v2 review/start events and separate immutable revision, approval, review, replay, and attempt persistence. Authenticated sender/target HostRefs must match the current session, and an exact correlated attempt start requires fresh Host admission before an accepted attempt record exists.
- Added v2 topology and lifecycle validation for participant/Host mismatch, stale sessions, hidden movement, exact Transfer revision/location tracking, review/admission correlation, v1/v2 replay isolation, whole-Plan Transform/Execute denial, restart interruption, and Burn deletion.
- Implemented Phase 3 generic Host-local admission over an exact stored approval, immutable Plan revision/hash, Plan participant/HostRef, and freshly revalidated HostSessionBinding. Admit decisions contain only exact Host-bound Search/Transfer work and bounded constraints; denial is fail closed and no admission is a step grant or modification authority.
- Added a Host-private generic managed-object acquisition/binding service for Search results, Inbox items, drag/drop, local selection, generated roots, and exact Transfer receipts. Current direct local selection and selected Search results use its v1 compatibility adapter without changing `selected_file`, Plan hashes, protocol payloads, or Transfer behavior.
- Implemented the additive Phase 2 Host identity contract: a Core-owned `HostRef` derived from the persistent installation identity, Plan-scoped role-neutral participant references, and an exact expiring `HostSessionBinding` over logical Hosts plus the current Layer 4 session/route.
- Added an optional backward-compatible HostRef exchange to the existing room join handshake and a nullable logical Host association on current `bridge_peers`. Legacy peers may omit the field; no Bridge Plan v1 payload, protocol message, schema, or revision hash changed.
- Implemented the Phase 1 UI-independent `HostRuntime` seam. The existing Layer 1–5 stores/services, Developer Terminal service, startup reconciliation, shutdown invalidation, explicit paths/configuration, Host event sink, and runtime task spawning now live behind one Host-owned container while Desktop behavior remains Tauri-backed.
- Implemented Developer Mode v0 as a human-only authority domain parallel to Layer 5: explicit local entry, exact current Host/session binding, remote human admission, a dedicated process-local `DeveloperTerminalGrant`, and the UI-independent `HostRuntime` terminal service.
- Added typed open/accept/deny/input/output/resize/exit/close terminal messages over the existing authenticated encrypted Bridge control transport. Terminal streams bypass ordinary room history and use bounded frames, buffers, sequencing, replay checks, and backpressure.
- Added real Unix PTY support with a bounded Host-owned shell policy and Windows PowerShell support through the native ConPTY backend.
- Added a minimal Bridge Developer Mode UI for Host selection, admission, terminal interaction, resize, state display, and explicit close.

### Fixed / Changed

- Replaced the requester-local readiness rejection with exact self-admission and direct local dispatch. Managed-provider requirements are evaluated only for managed steps authored on the local Host, managed dependencies consume authoritative native-v2 commits, and requester-destination Transfer receipts are validated before the shared requester commit becomes visible.
- Added a symmetric route-neutral `session_pair_ref` for cross-side readiness, prepared, result, and failure correlation while retaining the complete directional `binding_ref` as Host-private authority. Exact Bridge, Host, session, approval, attempt, revision, and local full-binding checks remain fail closed.
- Consolidated current architecture, managed-Agent contracts, contributor procedures, and concrete reference facts into their four canonical documents; merged the still-useful Developer Mode contract and removed superseded architecture documents and links.
- Renamed the roadmap-derived `phase5_v2.rs` module to `managed_execution.rs`, renamed `Phase5AuthorityStateV1` to the durable `EffectAuthorityStateV1`, and replaced the process-private `phase5-managed-resources` directory label with `managed-execution-resources`. Frozen attachment/hash-domain strings remain unchanged.
- Aligned canonical documentation with the 1.9.2 packaged baseline and the implemented 2.0 Native Agent and separate Generic Managed paths.
- Kept v2 Layer 4 delivery separate from topology and consent: the wire carries logical participants while the receiving Host resolves current session evidence locally. V2 admission creates no execution/step grant, so current Search/Transfer behavior remains exclusively on the unchanged v1 compatibility path.
- Kept generic Host admission distinct from requester approval, Layer 4 identity/liveness, capability facts, object presence, and one-use step grants. Bridge Plan v1 retains its exact receiver review/start authority path; mandatory native admission attaches with schema/protocol v2 rather than reinterpreting v1.
- Preserved Host-private paths and ObjectRef privacy while adding explicit logical object revision and Host location. Acquisition always creates revision 1 for a new root; only an explicit Transfer receipt may rebind an existing exact revision, and acquisition never creates N+1.
- Separated the Developer Terminal's existing session-derived v0 endpoint/binding types from durable `HostRef` and managed Plan identity. Terminal grants, wire messages, two-human admission, restart/disconnect/Burn revocation, and Layer 5 separation are unchanged.
- Added fail-closed Host identity/session validation for malformed or self-identical identity, conflicting association within one current peer session, stale/replaced routes, disconnect, restart recovery, expiry, and Burn. Route and liveness remain evidence only and create no admission or Layer 5 authority.
- Reduced Tauri coupling to the Desktop bootstrap and invoke adapter: `main.rs` now owns window/plugin/path discovery plus concrete event/task adapters, while discovery, transfers, Room Control terminal notifications, cleanup, startup, and Host background dispatch use the injected runtime interfaces. That Phase 1 extraction added no identity, admission, Multi-Host, Headless, Worker, or Transform/Execute behavior.
- Corrected intermittent Developer Terminal bottom-row clipping by aligning xterm padding with FitAddon geometry, observing the terminal container and active-layout wrapper, performing a bounded post-layout/font-ready fit and redraw, and suppressing duplicate remote resize reports.
- Replaced legacy unauthenticated remote `/leave` and `/burn` callbacks with a typed, encrypted, replay-checked current-session `bridge_membership.departure` event. An explicit local leave/Burn now removes only the authenticated departing peer from the survivor's current membership and revokes its runtime authority; it never remotely Burns the survivor's Bridge. Temporary disconnect continues to retain membership for reconnect.
- Serialized xterm input through a bounded one-writer queue. Rapid typing, key repeat, and allowed paste data now preserve byte order, coalesce small events, and use at most 8 KiB per wire frame without weakening strict receiver sequencing or replay rejection.
- Corrected the rapid-input failure path where concurrent Tauri invokes could deliver strict sequence frames out of order, a valid receiver rejection disconnected the controller session, and later concurrent input obscured the cause as `Developer terminal authority is unavailable`.
- Classified terminal flow-control, sequence, authority, and generic event rejection separately. The first rejected send stops queued input without hidden retry, so a rate or sequence failure is not overwritten by a later authority error.
- Replaced output visibility through the 1.6-second workspace poll with bounded local Tauri output events written directly to xterm. The existing bounded controller snapshot remains a resynchronization fallback; network terminal transport cadence is unchanged.
- Replaced the custom `<pre>` terminal renderer, ANSI-stripping regex, and manual React key map with lazy-loaded `@xterm/xterm` plus `@xterm/addon-fit`. xterm now owns VT rendering, the blinking cursor, native terminal key sequences, focus, wrapping, and resize fitting.
- Active Developer Mode now identifies the controlled Host, shell, and state while hiding fresh-request controls. Terminal input is forwarded only from focused xterm `onData`, and resize delivery is debounced.
- Traced the reported blank PowerShell viewport from ConPTY through the bounded output queue, typed Layer 4 frame, controller buffer, and xterm write boundary. No Pastey backend loss was identified; physical Windows confirmation remains pending.

### Security / Authority

- Layer 4 route/session state remains insufficient for terminal access. Terminal authority requires explicit human UI sessions on both devices and an exact Host/session/terminal-bound grant.
- Disconnect, leave, restart, expiry, explicit close, and Burn revoke terminal authority and terminate Host PTYs. There is no transparent resume.
- Developer Terminal authority cannot be derived from provider/model output, capability facts, Layer 5 Plan approval, or step grants; terminal effects do not claim managed revision lineage.

### Documentation

- Documented the Developer Mode v0 protocol, authority, platform, lifecycle, and evidence contract across the canonical architecture, Layer 4/5, and product status documentation.

### Known limitations

- Developer Mode v0 is desktop-to-desktop. There is no headless admission policy, persistent/resumable or multi-tab terminal, arbitrary process API, Agent access, or privilege escalation.
- A reported Mac-controller-to-Windows-Host run confirms interactive PowerShell and normal typing. Rapid-input ordering, paste, revocation-under-load, and the new local output-event path still require physical cross-device stress retesting; no complete physical Mac↔Windows/Linux E2E PASS claim is made.
- Native Agent source-level reliability and deterministic two-Host state validation are complete. Phase 2 still requires physical Mac ↔ Windows movement and failure-matrix acceptance, including real process/app-server, installed/authenticated Codex, APFS ↔ NTFS, LAN/socket, sleep/wake, firewall/platform, and packaged UX behavior. RC/stable acceptance remains unclaimed.
- Generic Managed Worker limits remain separate: a real external-provider and packaged Mac↔Windows Managed E2E PASS is unclaimed; Linux managed Process, Worker network tools, subagents, Headless Host, general cross-representation Transform E2E, and richer Generic Managed recovery remain incomplete. Windows local Managed Execute acceptance remains valid.
- Phase 6.7 adds a strict alias-only `CandidateSemanticPlanV2`, constrained local interpretation, an explicitly selected proposal-provider adapter, deterministic Rust Host/object/revision/Transfer resolution, and a bounded topology/movement review DTO. The new Tauri seam creates only an unapproved native-v2 Draft; Natural-v1 is unchanged and provider output cannot dispatch the Phase 6.6 execution path.

## 1.9.2 — 2026-08-26

### Added / Architecture

- Froze the current Layer 1–5 semantic and authority baseline around four managed primitives: Search finds, Transform modifies, Transfer moves, and Execute runs. Search and Transfer are executable; Transform and Execute remain reviewable Plan-framework intents only.
- Added canonical architecture documentation for the then-future Managed Workspace, PM/Worker separation, HostRuntime, HostRef, Host admission, managed-object binding, Host effect enforcement, Headless adapters, and separate Developer Mode authority domain.
- Added a shared Rust Layer 3 transfer-capacity boundary for ordinary and managed Transfers, with one global window budget, active-transfer limits, and runtime resizing based only on bounded resource facts.
- Made Layer 5 continuation primitive-neutral: immutable attempt state determines the next dependency-eligible authored Transfer, which is claimed atomically and dispatched exactly once. One attempt and its protocol state may contain multiple authored Transfer steps.

### Fixed / Hardened

- Made empty `0..N` capability projections valid throughout local creation, Room Control transport, receipt, and storage without indexing element zero, fabricating a fallback capability, or changing topology or authority.
- Removed the historical Transform-centric PipelinePrivate continuation path. Completing an explicit private landing no longer assumes Transform or lets Layer 1/4 choose the next semantic step.
- Replaced the weaker PipelinePrivate `File::open` plus digest path with the shared safe physical-file identity implementation, preserving BLAKE3 change detection and platform-specific no-follow/no-reparse identity checks.
- Closed the managed-Transfer resource-policy bypass so ordinary and Layer 5 Transfers reach the same Rust capacity-admission boundary before Layer 1 transport.
- Hardened framework-only admission at requester command, `BridgePlanStore::create_attempt_from_approval`, and receiver protocol boundaries. A reviewed Plan containing Transform or Execute is rejected as a whole before approval consumption, attempt/grant creation, candidate discovery, or other execution side effects.
- Preserved Burn as a receiver-Host-owned fail-closed authority cutoff, removed receiver absolute paths from cleanup/transfer logging, and kept Layer 3 projection metadata opaque and non-authoritative.

### Changed

- Replaced earlier readable-text, patch-specific, and Python/runtime-specific interpretations with one generic Layer 5 model: Transform carries reviewed modification intent for the current logical object and declares N→N+1 at the same Host; Execute carries reviewed execution intent and consumes the exact current revision.
- Kept movement explicit: PipelinePrivate is an authored intermediate Transfer landing, never Transform output, implicit fallback movement, or capability-driven topology repair.
- Unified Search, direct Transfer, and PipelinePrivate consumption around retained safe identity, exact locality, session correlation, one-use authority, restart, TTL, and Burn foundations.
- Clarified paired/previously connected devices as display identity rather than routeability, consent, or execution authority.

### Security / Authority

- Provider, model, renderer, capability facts, ObjectRefs, logs, and Layer 4 route/session state remain non-authority. One requester Review & Run binds the complete immutable semantic Plan.
- Only an explicit authored Transfer changes object location. Capability observations cannot select a Host, add movement, grant approval, or rewrite topology.
- Search and Transfer grants remain exact, one-use, and bound to Bridge/Plan/revision/attempt/step/session. Restart, expiry, disconnect, and Burn continue to invalidate process-local execution material as applicable.
- Layer 5 owns semantic eligibility; Layer 3 owns transport capacity; Layer 4 supplies current authenticated/session transport context; Layer 1 performs encrypted byte transfer.

### Documentation

- Recorded the agreed PM/Worker/Core, HostRuntime, HostRef, Host admission, object-binding, effect-enforcement, Headless, and Developer Mode boundaries in the canonical architecture documentation.
- Aligned the README, canonical architecture, layer contracts, reference, development workflow, public architecture copy, and release metadata with the current implementation.
- Recorded the post-1.9.2 structural freeze separately from intentionally evolvable two-party, `selected_file`, protocol-v1, Tauri-container, admission-policy, effect-envelope, Harness, Terminal, and Headless representations.
- Corrected the future dependency order to HostRuntime seam → HostRef contract → Host admission/object binding → Multi-Host schema/protocol v2 → effect/Terminal authority domains → concrete upper implementations.

### Known limitations / Not implemented

- There is no concrete Transform runtime, Execute runtime, Worker Harness, local-model runtime, Multi-Host Plan, Host admission implementation, Host effect enforcement, Developer Terminal, or Headless Host daemon.
- Current Plan/schema/protocol and Composer remain two-party and Search-first around `requesting_device`, `selected_device`, and `selected_file`.
- Automated tests and cross-compilation do not prove physical Mac↔Windows/Linux end-to-end behavior; no such physical E2E claim is made for this release.

## 1.9.1 — Layer 5 narrow product closure — 2026-07-08

> Historical record. The capability, consent, Return, TaskGraph, and sidecar details below were superseded and physically removed by the Bridge Plan architecture recorded in 1.9.2; they are not current product behavior.

### Added

- Added a static Agent Bridge capability registry and shared capability envelope for the existing Hello Peer / Hello Stdout capability lifecycle.
- Added the Layer 5 workspace capability `filesystem.find_file_candidates`, including `request_peer_file_candidates` action validation, PolicyGate bounds, selected-peer preview/execution wiring, receiver Allow once, a bounded Rust/Tauri metadata-only search executor, and typed redacted candidate results.
- Added the Layer 5 candidate-payload second-consent handoff path `transfer.request_candidate_payload`, including `request_peer_candidate_payload` action validation, selected-peer preview, capability-specific Allow once grant, exact execution-request binding, one-time consent consumption, receiver-local in-memory candidate resolution, existing transfer-queue handoff, Agent Bridge queue audit metadata, and typed `handoff_queued` results with zero transferred bytes at handoff time.
- Added Ask Bridge natural-v1 as the single Layer 5 natural-language product entry. Provider/model output is reduced to Search / Transform / Return; Search and Search -> Return are supported, while Search -> Transform -> Return is recognized but safely unsupported/future until bounded transform runtime exists.
- Folded Request file into Ask Bridge as a Search / Return plan using `filesystem.find_file_candidates`, manual candidate selection, second-consent `transfer.request_candidate_payload`, and existing transfer-pipeline handoff.
- Kept `runtime.hello_stdout` as diagnostic/test-only fixed runtime coverage and removed the user-facing Hello demo product path.
- Added the shared `OperationTimeline` product abstraction for Pastey lifecycle steps in Ask Bridge Search / Return.

### Documentation

- Added the naming guide later consolidated into `docs/reference.md`, covering schema versions, capability IDs, registry versions, protocol names, provider action kinds, executor kinds, and candidate-payload capability naming.
- Added the design-first capability-template and manifest architecture later consolidated into the Layer 5 and reference documentation, including autonomy profiles, approval policy/reviewer boundaries, existing capability adapter mapping, migration phases, and proposed tests.
- Added Phase 1-3 capability-template implementation scaffolding: static manifests for all existing Agent Bridge capabilities, additive template helper checks, a manifest test runner, and manifest-backed Hello Stdout execution binding without public contract changes.
- Added the Phase 4 `filesystem.find_file_candidates` template wrapper for common lifecycle checks while preserving filesystem-specific validation, receiver-local candidate storage, Rust discovery behavior, and metadata-only public results.
- Added the Phase 5 `transfer.request_candidate_payload` template wrapper for common lifecycle checks while preserving source discovery binding, receiver-local candidate resolution, existing queue handoff, Agent Bridge queue metadata, `handoff_queued` semantics, and metadata-only public results.
- Added the Phase 6 deterministic candidate-payload workflow that chains existing discovery and payload capabilities only after host validation, local search confirmation, receiver search consent, explicit user candidate selection, receiver payload consent, and safe queue handoff.
- Consolidated Agent Bridge capability contracts, provider behavior, Layer 5 workspace status, candidate-payload second-consent boundaries, receiver-local candidate resolution, queue handoff, manual smoke expectations, and validation guidance around the implemented file-candidate metadata search capability and payload handoff path.

### Changed

- Removed MIME-family bucketing from MicroFlowGroup grouping and diagnostics so small payload scheduling is based on scheduler/runtime facts rather than file format labels.
- Fixed Ask Bridge Search / Return target binding so embedded capability requests and preview envelopes use the canonical room-control selected peer ref without weakening validation.
- Made file-candidate search, candidate-payload Deny decisions, and diagnostic Hello denials terminal lifecycle states.
- Added automatic refresh/polling for active nonterminal Bridge detail Layer 5 operations while retaining `Check for updates` as fallback only.
- Corrected local and remote platform labeling so remote Linux peers do not inherit local `This Mac` display.
- Made long sent/received text and stdout/result blocks fully viewable and copyable while keeping truncation preview-only.
- Corrected Layer 5 status wording across architecture, Agent Bridge, transfer, release workflow, and validation docs to distinguish narrow 1.9.1 closure from full Agent/Jarvis completion.

### Removed

- Removed stale Layer 2 `recommended_roles` capability-probe output so Device Diagnostics remains factual and does not expose planner hints, peer rankings, or device recommendations.

### Unchanged

- No automatic file sending after Search, AI candidate auto-selection, trusted-session runtime behavior, shell/process execution, model-authored code, cwd/env/network target, open-ended tool runtime, MCP runtime, new data plane, broad natural-language automation, or binary-v1 protocol change is implemented. The receiver-local candidate store is in-memory only, clears on app restart, and does not make candidate ids paths or transfer authority.

## 1.9.0 — Agent Bridge capability slice — 2026-06-18

### Added

- Added the first Agent Bridge implementation: provider abstraction, deterministic mock provider, OpenAI-compatible cloud provider, redacted context snapshots, action-plan validation, deny-first PolicyGate, pending local confirmation, and a fixed Hello Peer request path.
- Added typed room-control events for capability preview, acknowledgement, denial, invalid/expired status, execution request, and execution result.
- Added an encrypted bounded room-control transport path with a current-session inbox, replay/expiry/rate bounds, delivery receipts, and queue integration separate from ordinary room text/file items.
- Added sender-side control-demand reservation that lowers the active data target from `8` to `7` while outgoing control work is queued/sending, then restores `8` after the quiet period and hot-adjusts supported active binary-v1 senders.
- Added receiver-side Peer PolicyGate review with explicit Allow once / Deny decisions, exact one-time consent binding, and consent consumption.
- Added the fixed bounded `runtime.execute_hello_template` capability executor, which returns exactly `hello peer!` through a typed execution result.
- Added room-scoped Agent Bridge UI for peer review, queue state, runtime reservation status, execution request/result state, and compact/advanced diagnostics.

### Changed

- Moved the active Agent Bridge workflow into the Room context while keeping provider kind, cloud base URL/model, runtime-memory API key, enablement, and redacted log level in Settings.
- Reframed Agent Bridge as a narrow end-to-end capability slice, not a completed general agent platform.

### Security

- Kept model output advisory only: the model proposes, the host validates, the user authorizes, and a bounded host-owned executor acts.
- Kept transport delivery separate from consent, trusted room membership separate from execution authority, and consent separate from reusable trust.
- Added redacted structured Agent Bridge lifecycle logging under `[pastey:agent-bridge]`; logs are audit mirrors only and do not become runtime state or authorization.

### Validation

- Added focused tests and runners for AI plan validation, room-control event schemas, control queue behavior, room-control transport, control-window runtime, receiver consent, Hello Peer execution, room-owned UI placement, and Agent Bridge logging.
- Added a deterministic control-lane contention harness that validates the production demand reducer, planner allocations, real Rust runtime-window update primitive, and room-control transport test stack.

### Documentation

- Established the project-layout specification and completion-scoring rules later consolidated into `docs/architecture.md` and the layer documents.
- Consolidated Agent Bridge documentation into current architecture/safety, room-control transport, capability-contract, and provider-configuration documents.
- Simplified the docs tree so stale phase reports and duplicate status narratives are removed; Git history remains the archive.

### Known limitations

- Agent Bridge currently implements one narrow Hello Peer capability slice. It is not a reusable general capability registry, arbitrary tool runtime, multi-step agent workspace, MCP integration, local LLM scheduler, durable trusted-room identity system, or reusable trust mechanism.
- Current room-control state is session-scoped and current-inbox based; it is not durable room history or durable authenticated peer identity.

## 1.8.0 — Dynamic MicroFlowGroup orchestration

### Added

- Added selectable live MicroFlowGroup modes: dynamic contention-aware one-window grouping as the default and fixed threshold grouping as a Developer Tools fallback.
- Added persisted `micro_flow_group_mode` configuration; mode changes affect later planner cycles without relaunching active transfers.
- Added Dynamic MicroFlowGroup planning that groups eligible tiny file-like work only under contention, uses bounded service-cost and group-size caps, and keeps at most one dynamic MicroFlowGroup window active.
- Added source-controlled transfer fixture manifests and a streaming deterministic generator for scheduler, MicroFlowGroup, chaos, and interruption smoke scenarios.
- Added persistent planner diagnostics for live mode, grouped children, skip reasons, fixed/dynamic candidates, dynamic capacity clamps, and runtime-window behavior.

### Changed

- Retired dynamic shadow as an active mode and made dynamic grouping the live default.
- Clarified weighted transfer planning around shared runtime-window capacity: active and runnable file-like transfers share the current target, with batch-relative requested-window allocation instead of independent per-transfer window claims.
- Preserved active transfer hot-window adjustment for supported outgoing binary-v1 senders while keeping the scheduler frontend-owned and file-like queue scoped.
- Clarified that Device Diagnostics remains current-session and advisory; `DeviceProfile`, `DeviceCapabilities`, `recommended_roles`, and benchmark results do not automatically command the scheduler.

### Fixed

- Hardened frontend-only MicroFlowGroup accounting so generated-payload serial groups do not finish with unaccounted children after successful child transfers.
- Preserved grouped-child reservations and terminal queue guards so late progress, cancelled items, burned rooms, and batch interruption do not duplicate or revive work.

### Validation

- Added planner replay scenarios, deterministic fixture generation, fixture-corpus documentation, and single-machine dual-instance smoke guidance.
- Documented how to identify the actual sender log by `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]` diagnostics.
- Kept single-machine smoke framed as lifecycle/logging evidence; two-machine release-build validation remains required for final throughput and cross-device conclusions.

### Documentation

- Consolidated transfer documentation under `docs/transfer/` for current transfer architecture, scheduler/MicroFlowGroup behavior, and validation/logging guidance.
- Folded `dev-fast` resource notes and Linux feasibility boundaries into transfer validation guidance.
- Added the static product website under `site/` with English and Simplified Chinese routes, release links, and Cloudflare Pages configuration.

### Unchanged

- MicroFlowGroup remains a scheduler/resource abstraction only. It does not change room items, binary-v1 frames, encryption, the Rust transfer hot path, receiver behavior, ACK/finalize/cancel/burn handling, Inbox behavior, JSON fallback, protocol negotiation, binary-v2, or file contents.
- Text sends remain immediate and outside the file queue.
- No general performance improvement claim is made for this release beyond the retained validation boundaries.

## 1.7.0 — Global Transfer Scheduler — 2026-05-30

- Added a frontend-owned weighted transfer scheduler for multi-file picker, drag/drop, and pasted-image sends.
- Added queue-item metadata readiness/cache so file-like items resolve display name, MIME type, size, modified time, and dedupe metadata before planner allocation.
- Added optional frontend queue-item correlation metadata to outgoing file progress events so concurrent sends can distinguish same-name/same-size queue items without changing transfer ids or chunk protocols.
- Added a pure weighted transfer planner module with deterministic allocation tests for lane budgets, held reasons, active budget reservation, runnable launch selection, duplicate-launch prevention, and requested-window invariants.
- Improved planner requested-window allocation so selected file-like transfers receive batch-relative size-weighted windows rather than mostly splitting by lane or size-class labels. Large-plus-small batches now request windows such as 7 plus 1, while similarly large batches split fairly within the global budget.
- Added planner-driven multi-worker execution for existing queued file-like transfers while preserving the existing `sendFileToRoom` / `send_file_to_room` single-file transfer path.
- Added `MicroFlowGroup` planner output for eligible tiny file-like queue items, including shadow reporting and scheduler-only serial dispatch where a group consumes one requested window and each child still uses the existing single-file transfer path.
- Added internal MicroFlowGroup runtime status tracking for queued, running, completed, completed-with-errors, cancelled, and interrupted serial groups.
- Added low-noise `[pastey:planner]`, `[pastey:micro-group]`, and `[pastey:runtime-window]` diagnostics that persist through the normal app log for manual validation without logging absolute file paths, including MicroFlowGroup no-group candidate summaries and runtime-window tracking/terminal summaries.
- Added a single-machine validation path with Tauri-free planner replay scenarios, fixed-vs-dynamic-shadow MicroFlowGroup diagnostics, and developer-only isolated app data/profile overrides for local dual-instance lifecycle smoke.
- Added planner and scheduler coverage for huge-plus-many-tiny allocation, serial MicroFlowGroup launch plans, one-window group invariants, group terminal state, and shadow grouping that leaves child runnable plans unchanged.
- Added optional sender-side `requestedWindow` plumbing through `sendFileToRoom`, Rust `send_file_to_room`, `send_room_file`, and transfer tuning. Planner-selected sends pass requested windows; env and effective Developer Tools overrides still take precedence, omitted values keep the window 8 default, and no receiver protocol fields changed.
- Added `npm run tauri:dev-fast`, backed by an optimized custom Cargo `dev-fast` profile, for faster local transfer-throughput testing.
- Documented that normal Tauri dev uses Cargo `dev` and can under-represent transfer throughput; packaged release builds remain the final production benchmark.
- Added a lightweight room queue panel with batch counts, active/queued/failed/completed/cancelled totals, multiple active transfer rows, and local queue cancellation controls.
- Hardened scheduler regression coverage for multi-active batch cancel, item cancel before and after transfer-id correlation, burned-room queue cleanup, active budget reservation, and late queue mutations against terminal items.
- Recorded partial Step 8 smoke validation: mixed dragged files completed, a 2.5GB GGUF completed around 108 MB/s average, burn behaved normally, and no obvious duplicate launch, progress cross-correlation, or terminal-state corruption was observed. Full benchmark and release-build validation remain separate.
- Added Phase 4A completion-only runtime window mutation for active outgoing binary-v1 sender transfers, including a sender-only runtime window handle, structured `update_transfer_window` no-op results, and frontend rebalance after planner-managed queue item completion.
- Recorded Phase 4A smoke validation for a 2.7GB plus 147MB pair: startup allocation was about 7 plus 1, the smaller window-1 transfer completed, completion-only rebalance updated the still-active larger transfer from runtime window 7 to 8 with `updated=true`, and the larger transfer completed without failed or duplicate chunks. This is smoke validation only, not full release-build benchmark validation.
- Kept text sending immediate and outside the file queue.
- Preserved the existing `sendFileToRoom` frontend wrapper and Rust `send_file_to_room` command as the authoritative single-file transfer path.
- Kept binary-v1 framing, JSON fallback, ACK behavior, receiver `.part` writes, finalize/cancel/burn handling, and terminal transfer reason mapping unchanged.
- Did not add retry/timeout adaptive downshift, stable cooldown recovery, speed-history heuristics, archive bundling, folder transfer, benchmark UI, backend-owned scheduling, binary-v2, substream multiplexing, or protocol changes.
- Kept file type as display metadata only; core binary file transport remains opaque and file-type independent.

## 1.6.0 — Device diagnostics foundation

- Added a lightweight Device Diagnostics foundation behind Developer Tools.
- Added local `DeviceProfile` and capability probing for device name, platform, OS, CPU, memory, GPU, power state, and a small whitelist of useful runtimes.
- Added local loopback diagnostics for raw memory/socket baseline and Pastey encrypted/framed pipeline overhead.
- Added peer benchmark backend support for trusted room peers without writing benchmark payloads to Inbox or disk.
- Clarified diagnostics semantics in the UI and README: loopback tests stay on the same device, peer tests measure LAN behavior, and only real transfers represent end-user file transfer speed.
- Refined diagnostics display to show concrete CPU, GPU, and runtime facts instead of internal routing role hints.
- Improved macOS and Windows device name, CPU, and GPU detection while avoiding serial numbers, MAC addresses, arbitrary commands, cloud upload, disk stress tests, and system-wide software inventory.
- Kept heuristically computed `recommended_roles` in backend data as internal advisory hints while hiding them from the main diagnostics card and leaving them disconnected from automatic routing or scheduler decisions.
- Added serialization, compatibility, parsing, benchmark discard, and diagnostics quality-label tests.

## 1.5.4 — Engineering cleanup and transport consolidation

- Centralized transfer window policy into `transfer_tuning.rs`.
- Removed duplicated transfer-window logic from `transfer.rs` and `config.rs`.
- Kept normal binary-v1 transfers on the established window 8 default.
- Preserved old `speed_limit_mbps` config compatibility without restoring user-facing speed limits.
- Cleaned temporary debugging logs and stale transfer scaffolding.
- Simplified Settings and Room page code after the transfer tuning changes.
- Updated README, transfer hot-path docs, and release workflow docs to match current behavior.
- Kept release workflow, binary-v1 transfer, legacy JSON fallback, burn/finalize, and nearby join behavior unchanged.

## 1.5.3 — Dev-only transfer tuning

- Normal transfers now run at maximum practical speed; Settings no longer exposes an MB/s transfer control.
- Defaulted binary-v1 transfers to window 8 after release LAN testing showed it as the best stable result.
- Converted transfer tuning into a developer-only Transfer Window control.
- Kept `PASTEY_TRANSFER_WINDOW_SIZE` for developer benchmarking.

## 1.5.2 — Speed policy and settings persistence

- Added early transfer-window benchmarking controls for binary-v1 transfer tuning.
- Added a debug transfer window override for benchmarking window 1, 2, 4, 8, and 16.
- Added transfer benchmark summary logs with effective window size, duration, throughput, and hot-path timing.
- Fixed the frontend Tauri argument name for config updates so Settings changes persist correctly.
- Verified bidirectional transfers after the speed policy fix.

## 1.5.1 — Transfer pipeline validation

- Replaced stop-and-wait binary-v1 chunk uploads with pipelined in-flight chunk uploads.
- Added out-of-order binary chunk handling with receiver-side file offset writes.
- Added received-chunk bitmap tracking so finalize still verifies full chunk count and total size.
- Safely ACKed duplicate chunks without double-counting received bytes.
- Reduced transfer hot-path overhead by throttling progress events and sampling non-error chunk logs.
- Removed per-chunk file flush after each receiver write.
- Added sampled sender and receiver timing logs for transfer hot-path profiling.
- Validated release transfer throughput improving from about 4.6 MB/s to about 91 MB/s in local LAN testing.

## 1.5.0 — Binary chunk protocol

- Added binary-v1 chunk frames for high-speed LAN file transfer.
- Reduced full 4 MiB chunk payload size from about 5.59 MB with JSON/base64 to about 4.19 MB with binary framing.
- Preserved legacy JSON/base64 chunk upload support for compatibility.
- Added protocol capability selection so updated peers use binary-v1 while unknown peers remain on JSON.
- Kept encryption, nonce behavior, chunk sizing, ACKs, burn/finalize lifecycle, and nearby discovery semantics unchanged.
- Added binary frame encode/decode validation and regression tests.

## 1.4.1 — Nearby join reliability

- Fixed nearby join requests using the advertised LAN HTTP endpoint instead of the UDP beacon source port.
- Added clearer nearby join diagnostics, including request URL, endpoint hit, response, UI prompt rendering, and timeout logs.
- Restored pending join prompts from backend state so Accept / Reject is not lost if the request arrives before the frontend subscribes.
- Prevented simultaneous nearby join attempts from deadlocking the UI.
- Added receiver-side terminal transfer reasons for cancelled, burned, left, interrupted, disconnected, and timed-out transfers.
- Mapped receiver-side interruption cases to clear sender messages such as "Receiver cancelled transfer," "Peer burned the room," and "Receiver stopped receiving."
- Added tests for advertised HTTP port regression and terminal transfer reason mapping.

## 1.4.0 — Automatic Nearby Antenna Discovery

- Added automatic LAN nearby-device discovery while the Pastey window is open.
- Added explicit nearby join requests with Accept / Reject before a room is created.
- Kept 8-digit room codes as the manual fallback for networks that block local discovery.
- Nearby device cards show device name, platform, availability, and version without showing IP addresses or ports.

## 1.3.3 — Destructive-transfer resilience

- Hardened interrupted transfer handling for app quits, peer disconnects, network drops, burn/cancel, and finalize/burn races.
- Startup recovery now marks stale in-progress items interrupted and removes stale receiver `.pastey-parts` files without scanning inbox contents.
- Kept terminal transfer UI states stable so late progress or ack events cannot revive completed, cancelled, burned, failed, or interrupted transfers.
- Aligned release versions and artifact naming so GitHub release assets match the tag/app version.

## 1.3.2 — Burn lifecycle cleanup

- Updated Burn Room semantics so tracked local room content is deleted.
- Burn now removes encrypted payloads, transient incoming files for that room, related `.part` files, room items, and active receiver transfer state.
- Inbox-saved received files are preserved when a room is burned.
- Preserves files from other rooms and skips paths outside allowed app-controlled roots.
- Added clearer burn error reporting for local deletion or permission failures.
- Added tests for same-room inbox cleanup, other-room preservation, missing paths, `.pastey-parts` cleanup, outside-root skips, and idempotent burn behavior.

## 1.3.1 — Chunked transfer stabilization

- Stabilized large-file transfer with a shared JSON chunk protocol, ACK-based progress, clearer transfer errors, and unique `.part` paths.
- Fixed duplicate file sends, incoming file metadata handling, and legacy payload decoding conflicts for completed chunked files.
- Fixed the Windows short-read bug so configured 4MiB chunks stay consistent with transfer metadata and final verification.
- Added local release-build log files and GitHub Actions release builds.

## 1.2.0 — UI and release polish

- Refined the monochrome glass-style UI and balanced the home screen layout.
- Matched Transfer room and Join room panels visually.
- Updated README wording and kept release artifacts small with build-size auditing.

## 1.1.0 — Large-file transfer

- Raised file support to 10GB with chunked encrypted LAN transfer.
- Added `.part` receiver writes, progress, speed, ETA, cancel, disk-space checks, and stale-part cleanup.
- Generalized file handling so unknown binary files use the same transfer path as common file types.

## 1.0.0 — Room-based transfer

- Reworked transfer flow from one code per item to one reusable room code per room.
- Added room items, recent rooms, manual burn cleanup, screenshot paste, drag/drop files, and Windows/macOS packaging.
- Stabilized local encrypted text/file/image transfer for small payloads.

## 0.1.0 — Initial MVP

- Built the first Tauri v2 desktop app with React, TypeScript, and Rust.
- Added local encrypted payload storage, SQLite metadata, UDP LAN discovery, and temporary HTTP transfer endpoints.
- Produced the first macOS `.app` / `.dmg` build.
