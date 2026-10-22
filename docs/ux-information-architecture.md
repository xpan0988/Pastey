# UX and information architecture

How the desktop UI is organized, and why. Product rules (authority, movement, review) are in [architecture](architecture.md) and [Layer 5](layers/layer-5-agent.md); this page only says where each one appears.

## Model the UI follows

- **This device** is the Host the user sits at. What applies to it everywhere lives in **Settings**.
- A **Bridge** pairs this device with one other device. Everything about that pairing lives **inside the Bridge**.
- Inside a Bridge there are three different kinds of work, kept apart because they carry different authority:

| Kind | Where | Authority | Review |
|---|---|---|---|
| Movement: text and files, either way | **Share** tab. Send by typing, + → Send files…, or drop on the window. Get a file with + → Get a file from <device>… | Send: this device's user. Get: the other device's shared search folders only | Get a file: review the Search, then choose the one file |
| Workspace sharing: this device's folders, for one Bridge | **Bridge details → Workspaces** → Share a workspace… (this device's own folder picker), then per folder: Sessions, excerpts, Agent requests, Allow without asking…, Stop sharing | This device's user only; the Bridge's own device, trusted once (inline Trust… there) | None to share; sharing allows nothing until a switch is on |
| Execution: Run Codex | **Agents** tab → New task → Shared by <device> (by name), Another folder on <device> (typed path), or On this device (movement) | The executing device's Accept, or its own unattended permission | The other device's Accept; workspace movement reviewed here first |
| Observation: Agent sessions | **Agents** tab → Continue a session (the list, and each session's view) | A share of that folder with Sessions on | None: read only, never starts, continues or stops a session |
| Continuation: an existing session, in itself | **Agents** tab → Continue a session → a session → Continue session | This device's user; on another device, a share with Sessions and Agent requests on, and its Accept | The other device's Accept for each remote continuation; the session must still end where you saw it |

### Inside the Bridge, not in Settings

A Bridge is the collaboration context the user already set up, so everything that concerns only that Bridge is done there. Before, letting the Mac read Windows's sessions meant leaving the Bridge for Settings → Permissions → Session reading, choosing the Mac again and choosing the folder again; running Codex there meant typing the Windows path on the Mac. Now:
- **Windows:** this Bridge → Bridge details → Workspaces → Share a workspace…, choose the folder with Windows's own picker, turn on Sessions (and Agent requests to let the Mac run or continue there). If the Mac is not trusted yet, the same place offers Trust… with the fingerprint, once.
- **Mac:** this Bridge → Agents → Agents on Windows. The header shows what Windows shares; New task offers it by name; Continue a session lists its sessions by itself; a session's view offers Continue session and then follows the run in place. Every missing permission is explained in place with the exact step on Windows, never as an unexplained grey button.
- Each device shows its identity to the Bridge's other devices once per connection while the Bridge is open (it grants nothing), so the other device can trust it without a step here.
- Settings → Permissions keeps the cross-Bridge view: every shared workspace (Stop sharing), unattended Codex access, and shared search folders.

### Agents: one device, one followed task, two things to do

People think "what are agents doing on that device", not "execution versus observation". The page answers the user's questions top down, in the order they arise:

1. **Which device?** "Agents on [device]", with its connection, and a sentence saying that you are on this device and that the work below runs on, is read from, and is allowed by that device. What it shares follows in one line; Access on <device> (collapsed unless something needs action there) lists each authority separately (pairing, running Codex, Codex's own approval prompts, reading sessions, continuing), with the exact steps there. There is no single trust switch, and nothing can be granted from this side.
2. **What is running?** The one task this Bridge follows, with where it is (Sent → Accepted there → Running → Result, or Started here → Running → Result on this device), what its outcome means, whether anything ran, and what to do next; then its approvals, reply, movement and repair actions. It is shown once: inside the session it continues while that session is open, otherwise here at the top, with Open the session. A finished task can be dismissed; one that needs resolution cannot.
3. **New task or Continue a session?** One choice, one view at a time (remembered per Bridge):
   - **New task** (badge: Execution) is the form: folder, task, and whether to continue the conversation Pastey keeps for you in that folder or start a new one. That conversation is itself a Codex thread, listed under Continue a session as "Started by Pastey"; to pick up a conversation from the Codex app or a terminal, Continue a session is the way.
   - **Continue a session** (badge: Read, then continue) lists what Claude Code and Codex have saved there, by itself once there is a folder to look in, each row with whether it can be continued from here (Can continue, Can try, Open elsewhere, Running, Only on that device, Can't continue). Opening one shows its view: first whether it can be continued and what happens if it is, then its state in words and who has it open, its history and the folder now, recent excerpts when shared, and the evidence one step away. Continue session replaces the form with the run's progress; when the run ends the session is read again, so it shows the new turn and its new end marker.

Why this shape rather than two stacked sections: the earlier page showed the device bar, the Run form, the running task, the session filters, the list and a long report on one scroll. Continuing a session meant scrolling past the Run form, and after Continue the button went grey while the run appeared in another section, far above; a refusal ("open in another Codex client") was a muted line there. Facts, Not known and Git evidence came before the Continue box. Now the decision comes first, the run stays where it was started, every terminal state says whether anything ran and what to do, and evidence is on request.

| Capability | Status |
|---|---|
| Run Codex on this or another device; continue Pastey's own conversation in a folder | Implemented |
| Workspace movement to another device and result return | Implemented |
| Share a folder with one Bridge, from inside it, with Sessions and Agent requests chosen per folder | Implemented (Mac ↔ Windows acceptance N) |
| Run Codex in a folder another device shares, chosen by name | Implemented: that device resolves its own path; same Review and unattended grants |
| Read-only discovery and inspection of Claude Code and Codex sessions, here or on another device that shares the folder with Sessions on | Implemented |
| Continue an idle Codex thread on this device, in the thread itself | Implemented: Codex's writer lock excludes other writers; checked against the turn you saw (real, macOS). A thread only Pastey's own idle conversation holds is continued after Pastey closes that session |
| Continue a Codex thread on another device | Implemented: needs a share with Sessions and Agent requests there, and that device's Accept per run (real Mac → Windows: refused while the Codex app had it open, continued once every Codex client was closed) |
| Continue an idle Claude Code session on this device | Implemented: `claude --bg --resume`, copy-on-held, checked against the entry you saw |
| Continue a Claude Code session from another device | Not enabled: a background session can outlive Pastey there (remote-continuation.md) |
| Observe or steer a turn the Codex app is running | Not possible through supported interfaces: its app-server is private; the writer lock only shows that it has the thread open |

Continuation is Execution, started from Observation: New task continues *Pastey's own* Codex conversation in a folder unless "Start a new conversation" is chosen, and a session's view offers **Continue session** for an existing Codex thread or Claude Code session. It runs in the session itself, only while the session still ends with the entry you saw, and is followed in that session's view. Each view says why when it cannot be continued (open in another client, a Pastey task running in it, changed, unreadable, or a Claude Code session on another device), and says "You can try" when Pastey cannot see whether a client has it open and Codex decides at the start. A continuation on another device also needs that folder shared with Sessions and Agent requests on, and that device's Accept, whose banner there names the shared folder it is for.

A session is saved history on its device, not a running program: the Codex app or a terminal quitting ends a process that had it open, not the session. Pastey reads that history through a read-only Codex of its own, so finding a thread never needs the app, and says where each "is it open?" answer comes from (Codex's lock for this thread on macOS; the device's list of running Codex programs on Windows; Pastey's own record of what it has open). Details: [Layer 5 → Sessions, processes and tasks](layers/layer-5-agent.md#sessions-processes-and-tasks).

Managed Plans arrive by event and are pinned above the Share thread while open (review, readiness, execution, terminal state), because they are reviewed movement across the Bridge.

Developer Mode is an elevated, rare action: Bridge **⋯ → Developer Mode** replaces the tabs until **Back to Bridge**.

## Navigation

```
Sidebar (device-wide)          Bridge (one pairing)                      Bridge details (inspector)
  New Bridge                     Header: device name → Device Details      Connection and pairing
  Inbox (received, all Bridges)  Tabs: Share | Agents                      Reach it on another network
  Bridges                        ⋯ Developer Mode                          Devices → Device Details
  Settings                                                                 Allowed on this device (Revoke)
                                                                           Let the other device trust this one
                                                                           Burn this Bridge…

Device Details (one device of a Bridge): Overview · Connection · Performance (Test connection) ·
  Agents · Trust & permissions (both directions) · Advanced (managed self-check)
```

The old global **Devices** and **Activity** routes and the always-on right context panel are gone: devices belong to one Bridge (its details), in-flight transfers show in the Share thread with progress, and received items are in the Inbox. The right panel no longer squeezes the Bridge in the default 760 px window; Bridge details open on demand (docked above 1080 px, overlay below).

## Settings: in context and device-wide

In context, inside a Bridge: connection, pairing, its devices and checks, what it may do here (each with Revoke), identity presentation, Burn, and every task's own parameters and actions (Run, Review movement, Approve and send, Stop, Reconcile, Retry, Abandon).

Device-wide, in Settings:

| Section | Contents |
|---|---|
| General | Appearance (System, Light, Dark; renderer-only), global shortcut, receiving folder (Open, Change…, Use default), Save received files, Save received images |
| This device | Identity and Replace key; device profile (Refresh); runtimes (Quick or Full check); local speed test (Pastey pipeline or raw memory, 1, 5 or 15 s) |
| Devices & trust | Trusted devices and the devices connected now (Trust… with a fingerprint check); paired devices (Forget) |
| Permissions | What other devices may do here, across Bridges: Unattended Codex access (add form one click away), Shared workspaces (every share with Stop sharing; created and changed in its Bridge), Shared search folders |
| Agents | Codex availability on this device (Check again); how approvals work; the Plan runtime and provider |
| Advanced | Developer tools, transfer grouping, transfer window (takes effect with Developer tools), logs and last error |
| About | Version, application data, updates |

Removed from view because the Host never reads it: "Burn defaults" (`auto_burn_after_download`). Its stored value is untouched. Placeholder rows that only said "Unavailable" or "Not projected" are gone. (An earlier revision of this page also listed "Save received files" and "Save received images" as unread; that was wrong, and both are back under General.)

## Device information: where each kind lives

| Kind | Where |
|---|---|
| This device's own facts and checks (identity, profile, runtimes, local speed test) | Settings → This device |
| One Bridge's connection and actions (pairing, address, devices, shared workspaces both ways, what else it may do here, Burn) | Bridge details |
| One device of a Bridge (session, measured link performance, what it runs, trust both ways, self-check) | Device Details |
| Long-lived trust and grants across Bridges | Settings → Devices & trust, Settings → Permissions |

Measurements are shown only after they are taken, with what they exercised and when. "Test connection" measures Pastey's pipeline over the Bridge in memory for about a second; the local speed test never touches the network. Pastey keeps no history of real transfer throughput, so it shows none.

## Feature preservation audit

| Original feature | Previous location | New location | Status |
|---|---|---|---|
| Device name, platform, power | Legacy Settings → General and Troubleshooting (before 2.0) | Settings → This device → Device | Restored |
| Capability probe (runtimes) | Legacy Settings → Diagnostics logging | Settings → This device → Runtimes | Restored (quick and full) |
| Local speed test (loopback, two modes, three durations, last result) | Legacy Settings → Troubleshooting; 2.0 kept a disabled "Run local test" | Settings → This device → Local speed test | Restored |
| Peer link throughput | 2.0 Devices → Check (inside the managed self-check) | Device Details → Performance → Test connection (read-only), and Advanced | Kept, now also without the managed E2E |
| Device diagnostics (identity, session, control, data path) | 2.0 Devices → Check | Device Details → Performance and Advanced | Kept |
| Managed readiness and E2E self-check | 2.0 Devices → Check | Device Details → Advanced | Kept |
| Bridge member list and liveness | 2.0 Devices page, right context panel | Bridge details → Devices | Kept |
| Pairing, new code, pair with code | 2.0 network panel | Bridge details → Connection | Kept |
| Private-network address | 2.0 network panel | Bridge details → Reach it on another network | Kept |
| Show this device | 2.0 network panel | Bridge details, Agents → Access, Device Details hint | Kept |
| Change receiving folder | Legacy Settings → Receiving | Settings → General → Receiving | Restored |
| Open receiving folder | Legacy and 2.0 Settings | Settings → General; Inbox → Open folder | Kept |
| Save received files / images | Legacy and 2.0 Settings | Settings → General → Receiving | Restored (briefly removed in error) |
| Transfer grouping and window | Legacy Settings → Transfer diagnostics (editable); 2.0 read only | Settings → Advanced | Restored as editable |
| Developer tools | Settings | Settings → Advanced | Kept |
| Logs folder, copy last error, check for updates | Legacy Settings → Troubleshooting; 2.0 "Unavailable" | Settings → Advanced and About | Kept |
| Activity (in-flight and recent transfers) | 2.0 Activity page | Share thread with live progress; Inbox for received items | Merged |
| Burn defaults | Legacy and 2.0 Settings | — | Removed: the Host never reads it |
| Find file | 2.0 composer mode | Share → + → Get a file from <device>… | Moved |
| Agent sessions | 2.0 Task mode card; then Sessions tab; then Agents → Sessions on <device> | Agents → Continue a session | Merged |
| Run with Codex | 2.0 Task mode card; then Agent tab; then Agents → Run a task | Agents → New task | Merged |
| Session report: Live state, Last turn, Facts / Inferred / Not known, ids | Agents → a session's report, always open | A session's view → Evidence and how Pastey knows | Kept, on request |
| The followed task (approvals, reply, movement, Reconcile, Stop, Abandon, Retry Return) | Agents → Run a task, above the form | Agents, above the choice; inside the session it continues while that is open | Kept, shown once |

Security information stays visible: every permission names the device, folder, Bridge and expiry; Revoke sits next to it in Settings and in that Bridge's details; Trust, Forget, Replace key and Burn each need a confirmation that says what happens.

## Find file

It was a top-level composer mode beside Send and Task, a leftover from when Pastey was a file-transfer tool. Its real job is the mirror of sending: get one file *from* the other device. Its use is occasional and always starts in a Bridge, next to sending. It is now **+ → Get a file from <device>…** in the Share composer (and in the empty Share view), opening a dialog. The Bridge Plan flow, its review, the shared-folder scope and Inbox delivery are unchanged.

## Run with Codex: what was wrong

The button was disabled when `busy`, an unresolved workspace movement, Codex not `available` on the target, the target missing a HostRef, or an empty **workspace** or **task** field. Session reading was never an input. In the reported case (Codex available on DESKTOP-DMI2L9P, a folder chosen on Windows) the folder chosen on Windows was a Session reading or Unattended access grant. Those never fill the Mac's free-text "Original workspace path…" field, which for a remote run must hold the folder's full path on Windows. Nothing said so.

Now:
- `src/lib/nativeAgentReadiness.ts` derives the action (run here, run there, review movement), the ordered requirements, the first blocker and what happens next. The card shows the checklist and blocker, and the button uses only that.
- "Where is the folder?" separates **Already on <device>** (type its path; no files move) from **On this device** (folder picker; reviewed movement).
- The target is kept by HostRef, so a reconnect shows "not connected" instead of falling back to this device.
- The remote capability is observed again every 15 s and on window focus, keeping the last answer visible.
- The started remote task stays bound to its device, so Stop and native decisions route there.

## Design system

Defined as tokens in `src/styles.css`; components use tokens only.

- **Colour.** Warm neutrals (`--bg`, `--bg-sidebar`, `--surface`, `--surface-2/3`), hairline borders, ink primary buttons (`--ink` / `--on-ink`), one accent (`--accent`) for focus and selection, and status tones (`--success`, `--warning`, `--danger`, `--info`) each with a soft fill. Dark and light differ only in tokens; `prefers-color-scheme` applies unless Appearance overrides it.
- **Type.** System sans for UI (13.5 px base), a serif for page and empty-state titles, mono for codes, fingerprints and paths. Scale: 11.5, 12.5, 13.5, 15, 19, 24.
- **Space and shape.** 4 px grid (4 to 48), radii 6 / 9 / 12 / 18, 32 px controls (28 px small), content width 720 px, settings width 640 px.
- **Components** (`src/features/workspace/ui.tsx`): Icon, Dot, Badge, Notice (neutral, info, warn, bad, ok), Switch, Segmented, Tabs, Field, MenuButton, Dialog, ListRow, Checklist, EmptyState.
- **Rules.** One primary action per surface. Destructive actions are outlined in red and confirmed in a dialog with a solid red button. A disabled primary button always has its reason beside it. Untrusted text (Agent output, session excerpts, request content) is plain text only. Long explanations sit under a section heading, not in every row; rarely used controls sit behind a disclosure (pairing when already paired, remote address, add-permission forms).
- **Responsive.** Above 1080 px Bridge details dock; 680 to 1080 px they overlay; below 680 px the sidebar becomes an overlay behind a menu button and Settings sections become a horizontal strip.
