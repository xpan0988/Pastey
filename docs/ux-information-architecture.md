# UX and information architecture

How the desktop UI is organized, and why. Product rules (authority, movement, review) are in [architecture](architecture.md) and [Layer 5](layers/layer-5-agent.md); this page only says where each one appears.

## Model the UI follows

- **This device** is the Host the user sits at. What applies to it everywhere lives in **Settings**.
- A **Bridge** pairs this device with one other device. Everything about that pairing lives **inside the Bridge**.
- Inside a Bridge there are three different kinds of work, kept apart because they carry different authority:

| Kind | Where | Authority | Review |
|---|---|---|---|
| Movement: text and files, either way | **Share** tab. Send by typing, + → Send files…, or drop on the window. Get a file with + → Get a file from <device>… | Send: this device's user. Get: the other device's shared search folders only | Get a file: review the Search, then choose the one file |
| Execution: Run Codex | **Agent** tab | The executing device's Accept, or its own unattended permission | The other device's Accept; workspace movement reviewed here first |
| Observation: Agent sessions | **Sessions** tab | The holding device's Session reading permission | None: read only, never starts, continues or stops a session |

Continuation stays inside Execution: Run with Codex continues *Pastey's own* Codex conversation in a folder unless "Start a new conversation" is chosen. Discovered sessions are never offered as continuable from here. A remote session says so ("Not supported from another device"). The Host has a local-only Claude Code continuation command (`continue_native_session`), but the UI deliberately does not offer it yet.

Managed Plans arrive by event and are pinned above the Share thread while open (review, readiness, execution, terminal state), because they are reviewed movement across the Bridge.

Developer Mode is an elevated, rare action: Bridge **⋯ → Developer Mode** replaces the tabs until **Back to Bridge**.

## Navigation

```
Sidebar (device-wide)          Bridge (one pairing)                      Bridge details (inspector)
  New Bridge                     Header: device name · status · code       Connection and pairing
  Inbox (received, all Bridges)  Tabs: Share | Agent | Sessions            Reach it on another network
  Bridges                        ⋯ Developer Mode                          Devices (+ Check)
  Settings                                                                 Allowed on this device (Revoke)
                                                                           Let the other device trust this one
                                                                           Burn this Bridge…
```

The old global **Devices** and **Activity** routes and the always-on right context panel are gone: devices belong to one Bridge (its details), in-flight transfers show in the Share thread with progress, and received items are in the Inbox. The right panel no longer squeezes the Bridge in the default 760 px window; Bridge details open on demand (docked above 1080 px, overlay below).

## Settings: in context and device-wide

In context, inside a Bridge: connection, pairing, its devices and checks, what it may do here (each with Revoke), identity presentation, Burn, and every task's own parameters and actions (Run, Review movement, Approve and send, Stop, Reconcile, Retry, Abandon).

Device-wide, in Settings:

| Section | Contents |
|---|---|
| General | Appearance (System, Light, Dark; renderer-only), receiving folder, global shortcut |
| Devices & trust | This device's identity and Replace key; trusted devices and the devices connected now (Trust… with a fingerprint check); paired devices (Forget) |
| Permissions | What other devices may do here: Unattended Codex access, Session reading, Shared search folders. Grants first with Revoke; the add form one click away |
| Agents | Codex availability on this device (Check again); how approvals work; the Plan runtime and provider |
| Advanced | Developer tools (the only config toggle the Host acts on), transfer tuning (read only), logs and last error |
| About | Version, application data, updates |

Removed from view because the Host never reads them: "Save received files", "Save received images", "Burn defaults". Their stored values are untouched. Placeholder rows that only said "Unavailable" or "Not projected" are gone, as are the Troubleshooting, Transfer diagnostics and Diagnostics sub-pages that held nothing else.

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
