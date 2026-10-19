# UX and information architecture

How the desktop UI is organized, and why. Product rules (authority, movement, review) are in [architecture](architecture.md) and [Layer 5](layers/layer-5-agent.md); this page only says where each one appears.

## Model the UI follows

- **This device** is the Host the user sits at. What applies to it everywhere lives in **Settings**.
- A **Bridge** pairs this device with one other device. Everything about that pairing lives **inside the Bridge**.
- Inside a Bridge there are three different kinds of work, kept apart because they carry different authority:

| Kind | Where | Authority | Review |
|---|---|---|---|
| Movement: text and files, either way | **Share** tab. Send by typing, + → Send files…, or drop on the window. Get a file with + → Get a file from <device>… | Send: this device's user. Get: the other device's shared search folders only | Get a file: review the Search, then choose the one file |
| Execution: Run Codex | **Agents** tab → Run a task | The executing device's Accept, or its own unattended permission | The other device's Accept; workspace movement reviewed here first |
| Observation: Agent sessions | **Agents** tab → Sessions on <device> | The holding device's Session reading permission | None: read only, never starts, continues or stops a session |

### Why one Agents tab, with two sections

People think "what are agents doing on that device", not "execution versus observation". Both start from the same choice, the device, and most visits want both: start something there, or see what is already there. Two top-level tabs made users choose a Pastey category first and pick the device twice.

The authorities stay apart inside the one tab:
- **Run a task** (badge: Execution) holds the form and the one task Pastey started and follows: its approvals, response, movement and recovery.
- **Sessions on <device>** (badge: Read only) lists what Claude Code and Codex already keep there. A session Pastey started is marked "Started by Pastey"; its live state is still followed under Run a task, so a run is never presented as two live things. Nothing in this list starts, continues or stops anything.
- **Access on <device>** lists each authority separately (pairing, running Codex, Codex's own approval prompts, reading sessions) with what is known here, what the other device decides, and the exact steps there. There is no single trust switch, and nothing can be granted from this side.

| Capability | Status |
|---|---|
| Run Codex on this or another device; continue Pastey's own conversation in a folder | Implemented |
| Workspace movement to another device and result return | Implemented |
| Read-only discovery and inspection of Claude Code and Codex sessions, here or on another device with its permission | Implemented |
| Continue a discovered Claude Code session on this device | Implemented in the Host (`continue_native_session`), not offered in the UI |
| Continue a session from another device | Not implemented (disabled by design, see Layer 5) |
| Continue a discovered Codex thread | Refused: no Codex interface shows a thread is free |

Continuation stays inside Execution: Run with Codex continues *Pastey's own* Codex conversation in a folder unless "Start a new conversation" is chosen. Discovered sessions are never offered as continuable from here. A remote session says so ("Not supported from another device"). The Host has a local-only Claude Code continuation command (`continue_native_session`), but the UI deliberately does not offer it yet.

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
| Permissions | What other devices may do here: Unattended Codex access, Session reading, Shared search folders. Grants first with Revoke; the add form one click away |
| Agents | Codex availability on this device (Check again); how approvals work; the Plan runtime and provider |
| Advanced | Developer tools, transfer grouping, transfer window (takes effect with Developer tools), logs and last error |
| About | Version, application data, updates |

Removed from view because the Host never reads it: "Burn defaults" (`auto_burn_after_download`). Its stored value is untouched. Placeholder rows that only said "Unavailable" or "Not projected" are gone. (An earlier revision of this page also listed "Save received files" and "Save received images" as unread; that was wrong, and both are back under General.)

## Device information: where each kind lives

| Kind | Where |
|---|---|
| This device's own facts and checks (identity, profile, runtimes, local speed test) | Settings → This device |
| One Bridge's connection and actions (pairing, address, devices, what it may do here, Burn) | Bridge details |
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
| Agent sessions | 2.0 Task mode card; then Sessions tab | Agents → Sessions on <device> | Merged |
| Run with Codex | 2.0 Task mode card; then Agent tab | Agents → Run a task | Merged |

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
