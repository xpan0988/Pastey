# Windows Claude Code acceptance (this device)

This procedure opens `agent.coding.claude_code_conversation` on Windows. Until every step passes, the capability reports `native_agent_platform_unverified` there, whatever Claude Code is installed: the gate is `WINDOWS_ACCEPTED_VERSION` in `src-tauri/src/native_agent/capability/claude_code/conversation.rs`, which is `None`. Background-session continuation (`agent.coding.claude_code`, `claude --bg`) stays unavailable on Windows; this procedure does not cover it.

What is already established, and how, is in [the validation record, section H](../research/claude-code-adapter-validation.md#h-windows-2026-11-09). This procedure covers what that could not: a signed-in Claude Code, real model turns and real tool processes, and the app itself.

## Prerequisites

- Windows 10 1809 or later (nested Job Objects), the Windows user who runs Pastey, a checkout of this branch, the Rust toolchain, Node and `npm ci` done.
- Claude Code installed for that user **from its official installer**, so `claude.exe` is in `%USERPROFILE%\.local\bin` or on `PATH`. The Claude desktop app's own copy is not used: Pastey does not look for it on Windows, and it reports itself signed out when run on its own.
- Signed in by you (Pastey never signs in):

```powershell
claude auth login
```

```powershell
claude auth status
```

  `loggedIn` must be `true`. `claude --version` must be 2.1.317 or later.
- A disposable folder Claude Code trusts (run `claude` in it once and accept the trust prompt), and a second, never-trusted folder containing `.claude\settings.json` with `{}`.

Run every command below from the repository root, in a PowerShell **not started from Claude Code** (a parent Claude Code session's variables change a child's sign-in). Record the outputs; none contains a secret.

## W-CC1 — Build and the deterministic Windows suites

```powershell
npm run build
```

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey -- claude_code::job:: conversation::windows_tests --test-threads=4
```

Expect `27 passed; 0 failed; 7 ignored`. These run real Windows processes (a fixture `claude.cmd`, PowerShell, `ping`, `conhost`) in the product's Job Object, but no Claude Code.

## W-CC2 — The installed, signed-in Claude Code through Pastey

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --bin pastey installed_claude_code_conversation_ -- --ignored --nocapture --test-threads=1
```

Expect 3 passed (the two `_helper` entries print nothing and pass):

| Test | Shows |
|---|---|
| `…runs_and_continues_on_windows` | A new session completes with its reply; the follow-up resumes exactly it and recalls the reply; both turns name one session; no `claude` process is left. |
| `…stops_with_its_tool_processes_on_windows` | Stop during a real Bash `ping` records `native_agent_cancelled`; the `claude` processes have ended and the folder is released. |
| `…ends_when_pastey_is_killed_on_windows` | A separate Pastey process is killed with `TerminateProcess` mid-turn. `claude` has ended within 3 s (printed). The restored task is never `native_agent_interrupted_on_restart`, `Failed` or `Completed`; the prompt is in the transcript once. |

Also record, from the first test's output, the `availability:` line (expected `Available`, `Claude Code <version> at <path>`).

## W-CC3 — Real tool processes under Stop and under a killed Pastey

W-CC2 checks `claude.exe` itself. Check its tool processes by hand once:

1. Start the stop test again with `--nocapture`. While it waits (it sleeps 8 s after Claude Code starts), run:

```powershell
Get-CimInstance Win32_Process | Where-Object { $_.Name -in 'bash.exe','ping.exe','claude.exe','node.exe' } | Select-Object ProcessId, ParentProcessId, Name
```

2. After the test passes, run it again. Expect no `ping.exe` and no `bash.exe` from that run.
3. Repeat for `…ends_when_pastey_is_killed_on_windows`.

Any process from the run that remains is a **failure**: containment did not hold.

## W-CC4 — The app

Run Pastey from this build, temporarily accepting the version under test: set `WINDOWS_ACCEPTED_VERSION` to `Some((2, 1, 317))` (or the version you measured, if newer), then

```powershell
npm run tauri:dev
```

| # | Do | Expect |
|---|---|---|
| 1 | Settings → Agents. | Claude Code: ready, "This device only". |
| 2 | Agents → This device → the trusted folder → New conversation → Claude Code → "Reply with the word ok". | Completed with the reply. No console window flashes at any point. |
| 3 | Reply "What did you reply last time?" | Completed, answers "ok"; Conversation continued. |
| 4 | New conversation: "Use the Bash tool to run `ping -n 120 127.0.0.1`, then say done." Stop after ~10 s. | "Stopped" within about 10 s. Task Manager → Details: no `claude.exe`, `bash.exe` or `ping.exe` from the run. |
| 5 | Same prompt, then Task Manager → End task on Pastey. Within 5 s check Task Manager. Relaunch. | No `claude.exe`, `bash.exe` or `ping.exe` from the run remains. After relaunch the task reads "Outcome unknown", then "Interrupted" from the transcript, never "Failed" or "Stopped when Pastey quit"; the folder is free afterwards; the prompt appears once in `claude --resume`. |
| 6 | The never-trusted folder: send anything. | "Did not start · nothing ran", naming `.claude/settings.json`. Run `claude` there, accept trust, send again: it runs. |
| 7 | While step 4's prompt runs again: open the same folder through a junction (`mklink /J C:\cc-alias <folder>`) and through another case spelling, and start a conversation in each; start one in a sibling folder. | Junction and case spelling: refused, the folder is held. Sibling: runs at once. |
| 8 | From a paired Mac: Device details → Agents. | Codex only. Claude Code is never offered to or invocable by a peer. |

## Passing

All of W-CC1 to W-CC4 pass. Then, in one commit:

- set `WINDOWS_ACCEPTED_VERSION` to the measured version;
- record the results (version, Windows build, dates, the timings printed) under section H of the validation record, keeping physical, fixture and not-run evidence apart;
- update R8 in the [Mac ↔ Windows acceptance](mac-windows-acceptance.md#r-claude-code-conversations-this-device-windows-boundary) and the renderer's wording for `native_agent_platform_unverified` if it should then name Windows.

Any failure leaves the gate closed: record what failed and stop.
