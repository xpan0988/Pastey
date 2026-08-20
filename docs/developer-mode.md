# Developer Mode v0

Developer Mode is Pastey's human-controlled remote terminal capability. It is a separate Host capability domain built above the current Bridge foundations. It is **not** a fifth Layer 5 primitive, a special Execute step, a managed object workflow, or an Agent escape hatch.

## Current implementation

The current desktop-to-desktop v0 flow is:

```text
human enters Developer Mode
  → selects one current connected Bridge Host
  → remote human explicitly allows or denies
  → process-local DeveloperTerminalGrant is consumed for PTY start
  → typed encrypted terminal frames use the current Bridge session
  → native Host-owned shell
```

The controller and Host both require an explicit process-local Developer Mode UI session. A connected route is necessary but insufficient. The remote Host creates the terminal grant only after its local user accepts the pending request. The grant is a dedicated type bound to the controller and target `HostRef`, exact current `HostSessionBinding`, Bridge, terminal session identifier, expiry, and one start. It is not a Layer 5 step grant and cannot create Layer 5 approval, revision, or object lineage.

Developer Mode v0 introduces a minimal UI-independent `HostRuntimeState` that owns `DeveloperTerminalService`. Tauri `AppState` still owns the wider desktop runtime container, paths, commands, event glue, and existing Layer 1–5 services. This is the first narrow HostRuntime seam, not a headless runtime extraction.

## Host identity and current-session binding

`HostRef` and `HostSessionBinding` are Developer Mode v0 contracts, separate from Layer 5's temporary `requesting_device` / `selected_device` representation.

- `HostRef` identifies the controller or target Host for this runtime generation.
- `HostSessionBinding` binds those Host refs to the exact current Layer 4 session refs and selected peer route.
- a route or liveness observation is not authority;
- a changed transport/session identity produces a different binding and old frames fail closed;
- the current v0 `HostRef` derives from current transport identity and is not yet the future durable Multi-Host `HostRef` contract.

Layer 5 Plan schema v1 is unchanged.

## Typed protocol and transport

The protocol family is `developer_terminal`. It defines:

- `developer_terminal.open_request`
- `developer_terminal.open_accepted`
- `developer_terminal.open_denied`
- `developer_terminal.input`
- `developer_terminal.output`
- `developer_terminal.resize`
- `developer_terminal.exit`
- `developer_terminal.close`

Every message carries the exact terminal session id, Host binding ref, controller Host ref, target Host ref, and a type-specific bounded payload. Input, output, and resize frames carry strictly increasing per-direction sequence numbers. Wrong Bridge/session/Host/binding/session-id, stale, replayed, out-of-order, oversized, and late frames are rejected.

Terminal messages reuse the existing authenticated and encrypted Room Control envelope, current peer route resolution, expiry, and transport receipt. They are delivered through a distinct typed branch directly to `DeveloperTerminalService`; terminal bytes never enter ordinary Room Control inbox history or Bridge item history. There is no second crypto, key-establishment, peer, or session stack.

Current bounds are deliberately narrow:

- 8 KiB maximum input/output frame;
- 64-frame bounded PTY output channel, providing backpressure to the blocking reader;
- 512 KiB bounded controller display buffer;
- bounded terminal event rate and burst cache;
- 30-minute UI-session lifetime, 2-minute admission request lifetime, and 30-minute terminal grant/session lifetime.

Terminal content and absolute local paths are not written to ordinary Pastey logs or history.

## Platform behavior

On Unix-like Hosts, Pastey opens a real native PTY. `$SHELL` is used only when it resolves to a Host-owned allowed shell path; otherwise the Host selects an allowed local fallback such as `/bin/sh`. The Host opens in the user's home directory and supplies its own terminal environment. The requester cannot supply a binary, argv, cwd, or environment.

On Windows, `portable-pty` uses the native ConPTY backend and Pastey selects `powershell.exe`. The requester cannot select another executable. Windows GNU cross-compilation verifies the code path, but this is not physical Windows runtime evidence.

Pastey performs no privilege escalation. The shell has only the privileges of the account running the Pastey Host process.

## Lifecycle

- **Open:** both humans explicitly enter/accept; the Host consumes one start grant and starts the PTY.
- **Deny:** the pending request becomes terminal and creates no grant or process.
- **Close/exit:** authority is revoked, the PTY/process is terminated or observed exited, and late frames are rejected.
- **Disconnect/leave:** Room Control teardown purges terminal state and terminates the Host PTY.
- **Restart:** all terminal UI sessions, grants, bindings, and PTYs are process-local and are not restored.
- **Burn:** Burn cuts authority off and purges terminal state/PTY through the same Bridge cleanup boundary. Late traffic cannot recreate it.
- **Reconnect:** there is no transparent resume; a new human admission and grant are required.

## Managed Workspace and Agent separation

Developer Terminal commands do not create Plan steps, Transform/Execute results, ObjectRefs, logical revisions, hidden Transfers, or managed result lineage. Human filesystem changes made in a terminal are external mutation from the Managed Workspace perspective and must be rebound/revalidated before future managed use.

No natural-v1, provider, planner, capability fact, Bridge route, or Layer 5 command can mint or consume `DeveloperTerminalGrant`. The Tauri terminal APIs require a dedicated process-local Developer Mode UI token in addition to exact active session correlation. Agent acquisition of terminal authority is forbidden, not a future inheritance path.

## v0 limitations

Developer Mode v0 is desktop-to-desktop and supports one terminal view per active controller session. It does not provide session persistence, transparent reconnect, tmux integration, terminal recording, multi-tab management, sudo automation, arbitrary remote process launch, custom shell/cwd/environment selection, a headless admission policy, or Agent access. The minimal renderer strips common ANSI control sequences for readable output; it is not a full terminal emulator. Explicit Bridge leave/stop and Burn revoke immediately; an otherwise silent idle network partition is observed on the next terminal/control frame, an existing route-lifecycle notification, or bounded expiry because v0 adds no terminal heartbeat.

Physical Mac-to-Windows/Linux cross-device behavior remains a release/manual evidence boundary.
