# Outcome

**Result: blocked, partial.** Each Agent finished, or nearly finished, one long first turn of investigation, design and source changes in its own worktree. They never coordinated through Pastey. claude's one message to codex was never delivered: codex's first turn never ended, because it waited on a native approval that Pastey's collaboration view did not show. Nothing was compiled by claude, nothing was committed by either Agent, and no mobile build exists. The user ended the experiment at 16:04 (60 min after the objective was delivered).

Labels: **[pastey]** Pastey fact, **[runtime]** native runtime report, **[agent]** Agent statement, **[observer]** my reading of the evidence. The patches are preserved under `raw/worktrees/`; the worktrees themselves were removed at the user's instruction, so nothing below was rebuilt by the observer.

## Problem interpretation

Both Agents read the repository independently, with no product brief, and reached **the same core interpretation**:

| | claude (Sonnet 5.5) | codex (GPT-6.2-sol) |
| --- | --- | --- |
| What a phone is | "A Bridge peer without Agent capabilities", foreground only [agent] | "A Bridge peer that requests work from a desktop Host" — the "mobile companion" [agent] |
| Agents | Runs none; starts/continues Codex and Claude Code conversations on desktops under the desktop's Review or Full Access [agent] | Registers zero local Agent adapters; requests desktop Agents; desktops keep review and locks [agent] |
| Pairing | Phone always joins, by the desktop's address + 8-digit code (unicast), because iOS broadcast needs Apple's multicast entitlement (marked "to verify") [agent] | Address + code over unicast, SPAKE2; "no multicast entitlement is needed on iOS" (cites Apple TN3179) [agent] |
| Files | In scope: phone → desktop as raw bytes ≤ 64 MB; desktop → phone into the app Inbox [agent] | Out of scope for this slice [agent] |
| Approving Reviews from a phone | Out of scope (would be a new authority path) [agent] | Not offered; desktop owns execution review [agent] |
| Platforms | iOS first (inferred from the repo's iPhone-hotspot measurements), then Android; asked the user [agent] | iOS, iPadOS and Android together; device qualification deferred [agent] |
| Evidence used | Repo: crate shape (binary crate, 59 modules in `main.rs`), desktop-only startup code, `discovery.rs` UDP broadcast/unicast, `bridge_lifecycle.rs` liveness, `send_file_to_room` path-based files, CSS breakpoints, Layer 1/4 docs that put mobile out of scope [agent, checked against the repo by observer] | Repo: Bridge pairing, Room Control, workspace shares, Agent lifecycle, `ui-bridge` (judged "a localhost test tool"); Tauri, Apple and Android docs (web search available to Codex) [agent] |

**Convergence [observer]:** strong on product shape (requester-only peer, address-based pairing, no local Agents, foreground only). **Divergence:** UI architecture (claude adapts the existing UI with platform checks and responsive CSS; codex adds a separate `mobile.html` / `src/mobile.tsx` / `MobileCompanion.tsx` entry and `mobile_companion.rs` that registers a reduced command set), crate split direction (claude: `lib.rs` 690 lines, `main.rs` 5; codex: `lib.rs` 10 lines, `main.rs` 578 plus `host_modules.rs` and `tauri_host.rs`), and file transfer scope.

## Implementation delivered (uncommitted, unverified by the observer)

### claude — `exp/mobile-claude` (22 tracked files, +284 −690; 5 new files)

- `docs/research/mobile-support.md` (151 lines): options considered (A requester peer — chosen; B phone browser remote UI — rejected, would need the protocol reimplemented in JS; C phone as full Host — not now), repo facts, lifecycle, pairing, files, Agents, crate structure, security, milestones M0–M4 with acceptance tests, open decisions.
- Rust: crate split into `pastey_lib` (`[lib] crate-type = staticlib, cdylib, rlib`) with `#[cfg_attr(mobile, tauri::mobile_entry_point)]`; tray, global shortcut, close handler, argument check and Claude Code lifeline behind `cfg(desktop)`; global-shortcut plugin and iroh `fast-apple-datapath` moved to target tables; `capabilities/default.json` desktop-only plus `capabilities/mobile.json`; new commands `write_temp_file_raw` and `get_local_network_addresses`; `cfg(mobile)` stubs; `discovery.rs` `local_private_ipv4_addresses` (+ test) and iOS/Android platform names.
- Frontend: `viewport-fit=cover`, safe-area/`dvh`/44 px targets/16 px inputs; `lib/platform.ts`, `lib/mobileFiles.ts`; address-based pairing flow; hidden desktop-only items; two frontend tests.
- Docs: every `cargo test --bin pastey` changed to `--lib` (development guide, Windows stage-5 script, ui-bridge README, two doc comments).
- **Verification: none.** Its own doc and message say so; the transcript shows `cargo`, `npm`, `rustc`, `rustup`, `xcodebuild` declined by Claude Code's headless permissions [runtime].

### codex — `exp/mobile-codex` (17 tracked files, +255 −131; 13 new files)

- `docs/platform/mobile.md` (44 lines): user steps, implementation notes, build/verify commands, what remains to qualify on devices.
- Rust: `lib.rs` mobile entry, `host_modules.rs`, `tauri_host.rs`, `mobile_companion.rs` (reduced IPC set, room server that rejects inbound work and omits file transfer), edits to `bridge_lifecycle.rs`, `host_runtime.rs`, `native_agent.rs`, `sessions/remote.rs`, `room_control.rs` (+ pair test), `transfer.rs`, `commands.rs`; `tauri.ios.conf.json`, `tauri.android.conf.json`, `Info.ios.plist`, `capabilities/mobile.json`.
- Frontend: `mobile.html`, `src/mobile.tsx`, `src/mobile.css`, `MobileCompanion.tsx`, `vite.config.ts` multi-entry; edits to `AgentWorkspace.tsx`, `BridgeWorkspace.tsx`, `tauri.ts`. README and Layer 4 doc updated.
- **Verification as reported:** `npm ci --offline`, `cargo check --locked --offline --bin pastey` (ran twice), `npm run build` ("the frontend build passes" — [agent]); a targeted `cargo test … mobile_companion_exchanges` **failed** [pastey live card / runtime]; then requested escalation to run a headless browser check (`node /private/tmp/pastey-mobile-browser-check.mjs`, saved as `raw/runtime/codex-escalated-browser-check.mjs`) — never answered. iOS/Android SDKs absent on this Mac [agent].

## Coordination that happened

- claude → codex (hop 1): asked codex to run checks on claude's worktree and proposed a split (claude: crate + capabilities + doc; codex: frontend mobile pass), "do not redo the lib.rs/main.rs split". **Not delivered**: codex never reached a turn boundary. By then codex had already done its own crate split.
- claude → user: phone priority and acceptance of the lib split; asked the user to run `cargo check` and `npm run build`. **Not answered** (user's choice).
- The user confirmed claude's summary decisions (H-1). codex never saw them (no later turn).
- codex sent nothing; it published no message or summary because its turn never ended.

## Demonstrable functionality

None demonstrated. No mobile target was built, no simulator or device was run, and no desktop regression run was made on either branch by the observer.

## Remaining defects and limits (from the evidence)

- claude's branch is uncompiled; its own list of likeliest faults: `cfg(desktop)` availability, the `tauri::ipc::Request` signature of `write_temp_file_raw`, dead-code warnings.
- codex's targeted test failed; cause not recorded before the stall.
- The two branches are mutually incompatible (both split the crate differently, both create `lib.rs` and `capabilities/mobile.json`); merging either into the other would need a decision neither Agent was able to make.
- Platform facts in both designs are partly unverified ("to verify" in claude's; codex cites docs but ran nothing on devices).
