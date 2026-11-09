# Windows Claude Code acceptance (this device)

This procedure opened `agent.coding.claude_code_conversation` on Windows. W-CC1 to W-CC4 steps 1–7 passed with Claude Code 2.1.319 on 2026-11-09 on a Windows 11 PC ([validation record](../research/claude-code-adapter-validation.md#h-windows-2026-11-09)). The gate was opened on 2026-11-10 by the user's decision **before step 8 (a paired Mac) ran**; step 8 is still to be run and recorded in section H12 of that record. The gate, `WINDOWS_ACCEPTED_VERSION` in `src-tauri/src/native_agent/capability/claude_code/conversation.rs`, is `Some((2, 1, 319))`. Run it again before accepting an older version, or after a Claude Code change to its headless interface. Background-session continuation (`agent.coding.claude_code`, `claude --bg`) stays unavailable on Windows; this procedure does not cover it.

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
| `…stops_with_its_tool_processes_on_windows` | Once Claude Code's Bash tool runs a real `ping -n 90`, Stop records `native_agent_cancelled`; the `claude` process and the `ping` have ended, none is left, and the folder is released. |
| `…ends_when_pastey_is_killed_on_windows` | Once the tool's `ping` runs, a separate Pastey process is killed with `TerminateProcess`. `claude` and the `ping` have ended within 3 s (printed). The restored task is never `native_agent_interrupted_on_restart`, `Failed` or `Completed`; this run's session holds the prompt once. |

The two tool tests write `.claude\settings.local.json` allowing `Bash(ping:*)` into their disposable folder under `src-tauri\target\`: headless, Claude Code denies a Bash call that would ask, and no tool process would run. This is Claude Code's own project setting; Pastey passes no permission.

Also record, from the first test's output, the `availability:` line (expected `Available`, `Claude Code <version> at <path>`).

## W-CC3 — No tool process survives

The tool tests above assert it. Confirm once by hand after W-CC2:

```powershell
Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'PING.EXE' -and $_.CommandLine -match '-n 90') -or ($_.Name -eq 'claude.exe' -and $_.CommandLine -match ' -p ') }
```

Expect no output. Any process from the run that remains is a **failure**: containment did not hold.

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
| 4 | New conversation: "Use the Bash tool to run `ping -n 120 127.0.0.1`, then say done." Stop after ~10 s. | "Stopping…" at once, while Claude Code ends; "Stopped" only once it has, within about 10 s. Task Manager → Details: no `claude.exe`, `bash.exe` or `ping.exe` from the run. |
| 5 | Same prompt, then Task Manager → End task on Pastey. Within 5 s check Task Manager. Relaunch. | No `claude.exe`, `bash.exe` or `ping.exe` from the run remains. After relaunch the task reads "Outcome unknown", then "Interrupted" from the transcript, never "Failed" or "Stopped when Pastey quit"; the folder is free afterwards; the prompt appears once in `claude --resume`. |
| 6 | The never-trusted folder: send anything. | "Did not start · nothing ran", naming `.claude/settings.json`. Run `claude` there, accept trust, send again: it runs. |
| 7 | While step 4's prompt runs again: open the same folder through a junction (`mklink /J C:\cc-alias <folder>`) and through another case spelling, and start a conversation in each; start one in a sibling folder. | Junction and case spelling: refused, the folder is held. Sibling: runs at once. |
| 8 | From a paired Mac, with the gate open on Windows: see the procedure below. | Codex only. Claude Code is never offered to or invocable by a peer. |

### Step 8: a paired Mac, with the Windows gate open

Step 8 checks remote filtering while Claude Code is **available** on Windows. With the gate closed it proves nothing, so it runs on an isolated Windows build whose gate is open, in a separate worktree. This branch's gate is never edited, and nothing here is committed.

You need:
- the Mac, with its usual Pastey (any 2.0 build that pairs with this branch);
- this Windows PC, with Claude Code signed in (`claude auth status` → `loggedIn: true`).

**On Windows**, in PowerShell, from this repository's root:

1. Make the isolated worktree:

```powershell
git worktree add --detach ..\pastey-step8 windows-claude-code
```

2. Open the gate in that worktree only:

```powershell
$f = (Resolve-Path ..\pastey-step8\src-tauri\src\native_agent\capability\claude_code\conversation.rs).Path; $t = [IO.File]::ReadAllText($f); $n = $t.Replace('WINDOWS_ACCEPTED_VERSION: Option<(u64, u64, u64)> = None;', 'WINDOWS_ACCEPTED_VERSION: Option<(u64, u64, u64)> = Some((2, 1, 319));'); if ($n -eq $t) { throw 'gate line not found' }; [IO.File]::WriteAllText($f, $n)
```

```powershell
git -C ..\pastey-step8 diff --stat
```

   Expect exactly one changed line in `conversation.rs`.

3. Build it:

```powershell
cd ..\pastey-step8; npm ci; npm run build; cargo build --manifest-path src-tauri\Cargo.toml --bin pastey
```

4. Run it on its own data, from a PowerShell **not** started from Claude Code:

```powershell
$env:PASTEY_APP_DATA_DIR = "$env:TEMP\pastey-step8-data"; .\src-tauri\target\debug\pastey.exe
```

5. In that window: Settings → Agents. Record that Claude Code reads **Available** ("This device only"). If it does not, the gate is not open and step 8 cannot count; stop here.
6. New Bridge → Create Bridge. Note the 8-digit code.

**On the Mac:** New Bridge → Join with a code → enter it. Wait until the Bridge shows Windows connected. Then record:

| # | On the Mac | Expect |
|---|---|---|
| 8a | The Bridge → Device details (Windows) → Agents | Codex only (in whatever state Windows reports for Codex). No Claude Code row. |
| 8b | The Bridge → Agents → Agents on: Windows | Only Codex offered for a new conversation on Windows; no Agent choice that includes Claude Code. |
| 8c | Settings → Agents | Claude Code on the Mac only (its own), never Windows's. |

**On Windows**, record Settings → Agents again: Claude Code is still Available. Then restore everything:

7. Close that Pastey window, then remove the worktree and its data:

```powershell
cd ..\Pastey; git worktree remove --force ..\pastey-step8; Remove-Item -Recurse -Force "$env:TEMP\pastey-step8-data"
```

```powershell
git worktree list; git status --short
```

   Expect no `pastey-step8` worktree, and a clean tree whose `WINDOWS_ACCEPTED_VERSION` is still `None`.
8. On the Mac, Burn or leave the test Bridge; it pairs with a build that no longer exists.

Step 8 passes when 8a–8c show no Claude Code for Windows while Windows itself showed it Available.

## Passing

All of W-CC1 to W-CC4 pass. Then, in one commit:

- set `WINDOWS_ACCEPTED_VERSION` to the measured version;
- record the results (version, Windows build, dates, the timings printed) under section H of the validation record, keeping physical, fixture and not-run evidence apart;
- update R8 in the [Mac ↔ Windows acceptance](mac-windows-acceptance.md#r-claude-code-conversations-this-device-windows-boundary) and the renderer's wording for `native_agent_platform_unverified` if it should then name Windows.

Any failure leaves the gate closed: record what failed and stop.
