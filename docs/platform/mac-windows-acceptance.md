# Mac ↔ Windows physical acceptance

One procedure for the physical evidence that automated tests cannot give: secure pairing, LAN Bridge operation, reconnection, representative Agent and file workflows, and native P2P reachability across networks. Each step says what to do, what must happen, and what to record.

Until a step is run on real devices and recorded, it is **untested**, whatever the automated suites say. Deterministic evidence is described in [development](../development.md).

## Scope and boundaries

- **Production paths:** pairing, LAN Bridge, overlay (Tailscale/WireGuard) reach, Agent and Transfer. These are exercised through the packaged app.
- **Native P2P traversal (iroh):** exercised only with the isolated `tools/net-probe`. iroh is **not** in the production app. Part F measures whether a production integration would be worth building; it is not a test of Pastey's Bridge.
- **Infrastructure:** n0's free public relays bootstrap the probe, under their development and hobby terms. Nothing paid is required.

## Prerequisites

- **Devices:** a Mac (Apple Silicon, current macOS) and a Windows 11 x64 PC.
- **Builds:** both run packaged builds of the same commit. On the Mac run `npm run build:checked`; on Windows run `npm run build:checked:windows` ([development](../development.md#setup-and-builds)). Record the commit and the version shown in each app.
- **Codex:** installed and signed in on the Windows Host for part E. Windows managed-execution setup is separate: [Windows managed execution](windows-managed-execution.md).
- **Networks:**
  - a shared Wi-Fi network;
  - a phone hotspot;
  - for C3, C5 and parts of F, a second fixed network (university, office, or a remote location).
  - Optional: Tailscale on both, for part G.
- **Test files, created on each device:**
  - a 1 GiB random file;
  - a small text file;
  - an image;
  - a folder of about 50 mixed files to share for Find file.

  macOS:

  ```bash
  head -c 1073741824 /dev/urandom > ~/pastey-accept-1g.bin
  ```

  Windows:

  ```powershell
  $b = New-Object byte[] 1MB; $r = [Random]::new(); $f = [IO.File]::OpenWrite("$HOME\pastey-accept-1g.bin"); 1..1024 | % { $r.NextBytes($b); $f.Write($b, 0, $b.Length) }; $f.Close()
  ```

- **File hashes:** check every transferred file. On macOS run `shasum -a 256 FILE`; on Windows run `Get-FileHash FILE -Algorithm SHA256`.

Record results in the template at the end. Never record codes, keys, device fingerprints beyond the 20 characters the app displays, public IP addresses, credentials, Host paths, or Agent or terminal content.

## A. Install and identity

| # | Do | Expect |
|---|---|---|
| A1 | Launch both apps. Allow the firewall prompt on Windows, and record the choice (private or public). | Both reach the home screen. |
| A2 | Settings → This device → This device's identity on each. | A 20-character fingerprint; note it for B and E. |

## B. Secure pairing (both devices on the same Wi-Fi)

| # | Do | Expect |
|---|---|---|
| B1 | Mac: New Bridge → Create Bridge. Windows: New Bridge → enter the Mac's code → Join. | Both show the Bridge as Connected; Bridge details (the panel button in the Bridge header) show **Connected · Paired** on both. |
| B2 | On a new Mac code, type a wrong code on Windows three times, then the right one. | Wrong codes: "did not answer" each time, and no Bridge appears on Windows. The right code then pairs. |
| B3 | After B1, enter the same code again on Windows, or on a third device if available. | Refused or no answer. A code admits one device. |
| B4 | Nearby flow. Windows: New Bridge → Nearby → Join on the Mac. Mac: Accept. | The Mac shows "Enter NNNN NNNN on <Windows>". Windows shows an "accepted" card asking for the code. Typing it pairs. Repeat once with **Reject**; Windows says rejected. |
| B4a | After B1 or B4, open New Bridge on Windows again. The Mac shows **Paired before**; choose **Connect**. Repeat once from the Mac towards Windows. | A new Bridge connects with no code and no Accept; the other device shows "started a new Bridge with this device". Busy does not block it. |
| B4b | On the Mac, open Settings → Devices & trust → Paired devices and **Forget** Windows. Then Connect from Windows again. | The Mac now shows the ordinary Accept prompt and code. Mac Bridges with Windows are disconnected and need pairing again. |
| B5 | Explicit re-pair. On the Mac's Bridge from B1: Bridge details → Pair again or pair another device → Pair another device. On Windows, enter that code under **New Bridge**. | Refused: "This device already has that Bridge". Then, on Windows, open that Bridge's details, type the code under **Pair with a code**, and pairing succeeds. |
| B6 | Legacy upgrade, if a pre-pairing build with an existing Bridge is available. Before upgrading, share a folder with that Bridge. | After upgrading both: the Bridge shows **Pairing required**, never reconnects on its own, and its history is visible. Re-pair it as in B5 (Show a new code on one side, Pair with a code on the other, both in Bridge details). Afterwards the folder share is gone and must be shared again; the history is still there. |

## C. LAN Bridge operation

| # | Do | Expect |
|---|---|---|
| C1 | Send text both ways. | Arrives once, exact. |
| C2 | Send the 1 GiB file Mac → Windows, then Windows → Mac. | Completes, and the SHA-256 matches. Record throughput from the transfer card. |
| C3 | Paste an image; send the small file to the Bridge. | Both arrive; the hashes match. |
| C4 | Start the 1 GiB send. Cancel it on the sender at about 30%. Repeat and cancel on the receiver. | Both sides show cancelled or interrupted; no partial file appears in the Inbox. |
| C5 | On Windows, share the test folder with this Bridge. On the Mac, in the Bridge's Share view, choose + → Get a file from <Windows>…, search a name inside it, and receive one file. | Only the chosen file arrives, hash intact. The device that ran the search changes nothing on Windows. |
| C6 | Developer Terminal: run the [physical checklist](../development.md#developer-terminal-physical-checks) in one direction. | As listed there. |

## D. Reconnection (same Wi-Fi unless stated)

After each step, send one text and the small file both ways.

| # | Do | Expect |
|---|---|---|
| D1 | Quit and relaunch Pastey on the Mac. | Connected again within about 10 s. No new code is needed. |
| D2 | Restart Windows. | Connected again after Pastey starts. |
| D3 | Turn Mac Wi-Fi off for 20 s, then on. | Reconnecting, then Connected. |
| D4 | Sleep the Mac 2 minutes, then wake it. | Connected within about 30 s of waking. |
| D5 | Start a 1 GiB send, then turn Mac Wi-Fi off mid-transfer for 20 s. | The transfer ends as interrupted, not as completed. A resend completes with a matching hash. |
| D6 | On Windows only: Settings → Devices & trust → Replace identity key…, then restart Pastey on Windows. | The Bridge stays disconnected: the Mac refuses the new key on reconnect. Re-pair: on the Mac, Bridge details → Pair another device; on Windows, that Bridge's details → Pair with a code. Then it is Connected. |

## E. Agent workflows (Codex on Windows, requested from the Mac)

| # | Do | Expect |
|---|---|---|
| E0 | On Windows, check that `codex --version` works in a new terminal. In Pastey on the Mac, open the Bridge's Agents tab and choose Windows as the Device. | "Codex is available on <Windows>". If not, the card names the failing layer. Run `powershell -ExecutionPolicy Bypass -File .\scripts\diagnose-windows-codex.ps1` on Windows and record its report: it masks the user name, but review it before sharing. |
| E0a | Only if the ChatGPT desktop app with Codex is installed on Windows. Run the diagnosis script and record its "Codex bundled with a desktop app" section. Then, with no Codex CLI on PATH (or in a Windows account without one), open the Agents tab of a Bridge on Windows (Device: This device) and choose Check again, or Settings → Agents → Check again. Run E1 once. | The section shows a manifest with `layoutVersion=1`, an existing entrypoint, a working `--version`, and `app-server` help. The card says Codex from the ChatGPT desktop app is ready. E1 completes. If the card instead says not installed or incompatible, record its text and the script's report. |
| E1 | From the Mac, Agents tab: Device Windows, “Already on <Windows>”, type the folder's full Windows path and a small task, then Run with Codex. Before typing, the card lists what is still needed and the button says why it is disabled. | Windows shows the Agent request. Accept runs it; the reply returns to the Mac's task card, which names Windows. |
| E2 | Repeat and choose Deny. | Nothing runs; the Mac shows that it did not start. |
| E3 | Workspace movement: Agents tab, Device Windows, “On this device”, choose a small folder → Review movement, Approve and send on the Mac, Accept on Windows. | The workspace lands, Codex runs, the result returns and applies, and the Mac's original folder is unchanged until it is applied. |
| E4 | Cancel a running task from the Mac. | Windows stops it, and both cards show cancelled. |
| E5 | Turn Mac Wi-Fi off during a task for 20 s, then reconnect. | Reconcile shows the true outcome. Nothing runs twice. |
| E6 | Unattended access. On Windows, Settings → Devices & trust: compare the Mac's fingerprint (A2, now under Settings → This device) and trust it; then Settings → Permissions → Unattended Codex access: grant one folder. Start a task from the Mac. | It runs without Accept. Revoke the grant: the next task waits for Accept again. |
| E7 | Re-pair this Bridge with a **different** device's code (or after D6), then repeat E1 from that device. | Earlier tasks, conversations and folder shares from before the re-pair are gone. Unattended runs need a grant for the new device. |

## F. Native P2P traversal across networks (isolated probe)

Run the `tools/net-probe` matrix in [its README](../../tools/net-probe/README.md):

| Cell | What it covers |
|---|---|
| C1 | Same Wi-Fi |
| C2 | Home ↔ phone hotspot (CGNAT) |
| C3 | Home ↔ university or office |
| C4 | Hotspot ↔ hotspot |
| C5 | Sydney ↔ mainland China |
| C6 | UDP blocked |
| R1 | Network switch |
| R2 | Sleep and wake |
| R3 | Peer restart |
| M1 | Two dialers into one listener |

Run every cell in both directions, with `--http`, so the room-shaped HTTP-over-QUIC check runs on real paths. Summarize with:

```bash
python3 -I tools/net-probe/summarize.py tools/net-probe/results/*.jsonl
```

These are the decision bars for a later production integration. They are measurement, not a test of the Bridge:
- C2 reaches direct in at least 8 of 10 runs within 10 s.
- 100% of digests intact.
- Relay-only runs report `relay` truthfully.
- Every R-cell outage recovers or is reported open.
- The unlisted key in M1 is rejected.

## G. Cross-network Bridge today (overlay, production path)

| # | Do | Expect |
|---|---|---|
| G1 | Tailscale on both; the Mac on a phone hotspot, Windows at home. Mac: Create Bridge. Windows: Join with "another network" and the Mac's Tailscale name. | Pairs. A 256 MiB file transfers with matching hashes. |
| G2 | Set the remembered address on both. Switch the Mac's network, or quit and relaunch Pastey. | Reconnects through the overlay address. |

## H. Burn and departure

| # | Do | Expect |
|---|---|---|
| H1 | Burn the Bridge on the Mac. | It disappears from the Mac. Windows shows the peer departed. Windows keeps its own copy and history, cannot reconnect, and needs a new pairing. |
| H2 | On Windows, Burn its copy. | The Bridge is removed; files saved to the Inbox remain. |

## I. Remote session reading (Windows sessions seen from the Mac)

Prerequisites:
- B1 or B4 done; the Mac trusted on Windows as in E6.
- On Windows, a folder (for example `C:\Users\<you>\Projects\Demo`) with at least one Claude Code session and, if Codex is installed, one Codex thread.
  - Make a Claude Code session by running `claude` there once and asking one question.
  - Make a Codex thread with `codex` or the ChatGPT desktop app in that folder.

| # | Do | Expect |
|---|---|---|
| I1 | On the Mac, Bridge → Agents tab → Device Windows → Look for sessions, before any permission. | "Windows has not allowed this device to read its sessions…", with where to allow it. No session is listed. Access on Windows → Read sessions now says Needs action, with the steps on Windows. |
| I2 | On Windows, open this Bridge → Bridge details → Workspaces → Share a workspace…, choose the Demo folder, turn on Sessions (excerpts off), 1 day → Share. Then repeat I1 on the Mac. | Windows's sessions in Demo are listed, with folder name "Demo" and no Windows path anywhere. Claude Code sessions show "Last turn ended" or "Running". A Codex thread shows "Unknown" while the ChatGPT desktop app or a `codex` terminal runs on Windows. Record what each row says. |
| I3 | On the Mac, open one Claude Code session. | The report shows status, live source (`claude agents --json`), last turn, counts, changed files relative to Demo, Git facts, and facts, inferences and unknowns. It says Windows shares statuses only: no prompt or reply text. |
| I4 | On Windows, start a Claude Code turn in that session (`claude --resume <id>` in Demo, or the desktop app). While it runs, look again from the Mac and open it. | It shows "Running" while the turn runs, and "Last turn ended" after it. The report never claims done while running. |
| I5 | On Windows, turn Sessions off for Demo (Bridge details → Workspaces), then Stop sharing it, then (after sharing again) revoke the Mac under Settings → Devices & trust and trust it again. Look again from the Mac after each. | Each time: "hasn't shared any session-readable workspace". After re-trusting, Demo is not shared again until Windows shares it. |
| I6 | On Windows, share again with Sessions and excerpts on. Open the session from the Mac. | Up to 4 short excerpts, replies labelled as the agent's own account, any path shown as `<path>`. |
| I7 | Burn the Bridge on either side, pair again, and look from the Mac. | "Hasn't shared": shares do not survive Burn. |

Record which rows passed. Do not paste session titles, prompts or paths into the record.

## J. Desktop UI (both devices)

The UI was reorganized around Bridges ([UX and information architecture](../ux-information-architecture.md)). These rows check what automated tests and a browser preview cannot: the real windows, system pickers and Tauri events.

| # | Do | Expect |
|---|---|---|
| J1 | Open Pastey at its default 760 × 760 window, then widen past 1080 px and narrow below 680 px. | At 760 px the sidebar and the Bridge fit with no overlap. Above 1080 px Bridge details dock on the right; below, they open as an overlay. Below 680 px the sidebar opens from the menu button. |
| J2 | Settings → General → Appearance: Light, Dark, System. Restart Pastey. | Each theme applies at once and survives the restart; System follows the OS. |
| J3 | Drag two files from Finder or Explorer onto a Bridge's Share view. | The composer shows "Drop to send…"; both files are sent to the selected device, like + → Send files…. Nothing happens on the Agents tab. |
| J4 | Mac, Agents tab: Device Windows → Another folder on Windows, and leave the path empty. Then type the path, then the task. | The checklist and the line beside the button say what is missing; the button enables only when nothing is. |
| J5 | With the Agents tab open on the Mac, quit Pastey on Windows, wait 20 s, open it again. | The card says Windows is not connected, then, without a click, that Codex is available again. The device choice stays Windows throughout. |
| J6 | On Windows, while the Mac's Agents tab is open, rename `codex` off PATH (or sign out) and back. Switch focus to the Mac window. | Within about 15 s, or at once on focus, the Mac shows the new availability and the button follows it. |
| J7 | Burn a test Bridge from its details. | A confirmation names the Bridge and what is removed. Cancel keeps it; Burn removes it. |

## K. Windows executor crash diagnosis

Use this for any crash of Pastey on the executing device, including one that already happened. Pastey has no auto-restart of its own; record how it came back (relaunched by you, by sign-in, or by itself).

| # | Do | Expect / record |
|---|---|---|
| K1 | On Windows, in PowerShell: `Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=(Get-Date).AddDays(-7)} \| Where-Object { $_.Message -match 'pastey' } \| Format-List TimeCreated, ProviderName, Id, Message` | Record each "Application Error" (Id 1000) and "Windows Error Reporting" (Id 1001) for `pastey.exe`: the time, faulting module and exception code. `0xc0000409` is a Rust panic (release builds abort on panic); `0xc0000005` an access violation; `0xc00000fd` a stack overflow. A crash of `msedgewebview2.exe` is the WebView, not Pastey's core. |
| K2 | `Get-ChildItem "$env:LOCALAPPDATA\Microsoft\Windows\WER\ReportArchive" \| Where-Object Name -like '*pastey*'` and open `Report.wer` in the newest folder. | Record the exception code and module. Do not share the dump itself. |
| K3 | `Get-Content "$env:LOCALAPPDATA\pastey\logs\pastey.log" -Tail 300` (also `pastey.log.1`). From this build on, the log has `[pastey crash] event=panic thread=… location=… message=…` for a panic, `[pastey lifecycle] event=previous_run_ended_uncleanly` on the next start after any unclean exit, and `[pastey native-agent] event=…` for each task step (received, review accepted, turn starting, turn ended, interrupted by restart). | Record the lines around the crash. They name tasks by id only; no folder paths or prompts. |
| K4 | Right after a crash, before restarting Pastey: `Get-Process codex, node -ErrorAction SilentlyContinue \| Format-Table Id, ProcessName, StartTime`. | Whether Codex outlived Pastey. Expected: none (closing Pastey's pipe ends the app-server). If one remains, record it and whether the test folder still changed afterwards. |
| K5 | Reproduce E1 with this build, once with Accept on Windows and once under an unattended permission. | Both complete. If it crashes, K1–K4 now have the cause. On the Mac the task shows the indeterminate notice; Reconcile, then Stop. Nothing is run again automatically. |
| K6 | Start a task from the Mac, switch to another Bridge and back (or reload with ⌘R). | The running task card is still there, names Windows, and Stop still works. |

## L. Device details and diagnostics

| # | Do | Expect |
|---|---|---|
| L1 | Click the device name in a Bridge header, or a device in Bridge details. | Device Details opens with Overview, Connection, Performance, Agents, Trust & permissions and Advanced. |
| L2 | Performance → Test connection. | About a second, then a measured average and peak MB/s, latency, quality and "Measured just now", labelled as Pastey's pipeline over this Bridge in memory. Record the numbers. |
| L3 | Advanced → Run managed self-check. | Managed readiness and the E2E result (PASS, BLOCKED or FAIL), as before under Bridge Devices → Check. |
| L4 | Settings → This device: Refresh, Full check, Run local test (each mode once). | Device facts, runtimes with versions, and a local-only measurement labelled "no network". |
| L5 | Settings → General → Receiving folder → Change…, receive a file, then Use default. Turn Save received files off and receive a file. | The file lands in the chosen folder; with saving off it is not put in the receiving folder. |

## M. Native session continuation (Codex threads, Claude Code sessions)

Prerequisites: Codex 0.163 or later on both devices (CLI and, if installed, the ChatGPT desktop app); on Windows, Python 3 for the probe; Windows shares the Demo folder with this Bridge with Sessions and Agent requests on (I2, then turn on Agent requests).

| # | Do | Expect / record |
|---|---|---|
| M1 | On Windows: `python -I tools\codex-session-probe\writer_lock_probe.py (Get-Command codex).Source`, once with the ChatGPT app open on a Codex thread and once with `--cross-process`. | `b_resume_while_a_holds` and `resume_of_thread_held_elsewhere` say "already has an active writer"; after A exits, B resumes. This is the Windows proof of the single-writer lock the remote path relies on. Record the JSON (ids are shortened). |
| M2 | Mac, local: Agents → Device: This device → choose a folder with a Codex thread you made in the ChatGPT app → Look → open it. With the thread still open in the app, read the report and its Continue section. | Live state "Open, idle" or "Running", source "Codex thread writer lock"; Continue explains that the thread is open in another Codex client. |
| M3 | Close that thread in the app (open another), Look again, open it, Continue with "Reply with the word ok". | Under Run a task: one run on this device, "continued conversation", Completed with the reply. In the ChatGPT app, the thread shows the new turn. The thread id is unchanged. |
| M4 | Repeat M3 but, before pressing Continue, add a turn to the thread in the app. | "This session changed since you looked at it, so nothing ran." Nothing new appears in the thread. |
| M5 | Mac → Windows: Agents → Device: Windows → Look → open a Codex thread in Demo that the app on Windows does not have open → Continue. On Windows, Accept. | The Windows request banner says "continuing an existing thread". The run completes on Windows in the same thread, and its reply returns to the Mac card, which names Windows. |
| M6 | M5 again, but Deny on Windows; then once with the thread open in the Windows app, Accept. | Deny: nothing runs. Open in the app: "open in another Codex client … nothing ran." |
| M7 | On Windows, turn Agent requests off for Demo (or Stop sharing it), then M5 from a report opened before. | Refused; nothing runs on Windows. The Mac card shows the task as indeterminate (the existing rule for a refused invocation): Stop it. |
| M8 | During M5's run, quit Pastey on Windows (Task Manager). | The Mac shows the indeterminate notice. After Windows restarts: no `codex` process from that run remains (K4), the thread is no longer locked (M2-style look shows it free), and nothing runs again on its own. |
| M9 | Claude Code, local: in a folder with an idle Claude Code session (not open in the desktop app or a terminal), Continue with a short prompt. Then try a remote Claude Code session from the Mac. | Local: completes in the same session id. Remote: Continue is unavailable with the reason (background sessions can outlive Pastey there). |

## N. Bridge workspaces: share, read, run and continue without leaving the Bridge

The target flow, end to end, on real devices. Prerequisites: a Mac ↔ Windows Bridge (B1 or B4); Codex 0.163 or later on Windows; on Windows a folder `PasteySessionTest` with one Codex thread made in the ChatGPT desktop app (then switch the app to another thread, so it does not hold this one). Do not open Settings on either device during N1–N9.

| # | Do | Expect / record |
|---|---|---|
| N1 | Open the Bridge on both devices. On Windows: Bridge details → Workspaces. | Within a few seconds Windows shows the Mac in "Shared from this device": either "Trust MacBook…" (first time) or nothing to do. It never says the Mac has not shown itself while the Mac has the Bridge open. |
| N2 | If asked: Trust MacBook…, compare the fingerprint with the Mac's Settings → This device, It matches. Trust. | Trusted once. Nothing is shared or allowed yet. |
| N3 | Windows: Share a workspace… | The Windows folder picker opens (a Windows dialog, Windows paths). Choose `PasteySessionTest`. |
| N4 | In the dialog turn on Sessions and Agent requests, 1 day → Share. | The row shows the folder name, its Windows path (only here), "Sessions and agent requests", and switches. |
| N5 | Mac: Agents, device Windows. | "Shared by Windows: PasteySessionTest (sessions and agent requests)". Access on Windows: Read sessions Ready; Continue decided there. No Windows path anywhere on the Mac. |
| N6 | Mac: Run a task → Shared by Windows → PasteySessionTest, task "List the files", Run with Codex. On Windows, Accept. | Runs in `PasteySessionTest` on Windows without typing a path; the reply returns to the Mac. |
| N7 | Mac: Sessions on Windows → Look for sessions → open the Codex thread → Continue session with "Reply with the word ok". | Windows shows the Review ("continuing an existing thread"). |
| N8 | Windows: Accept. | Codex continues in the same thread (the ChatGPT app shows the new turn when you open the thread); the Mac card completes with the reply. |
| N9 | Windows: turn Agent requests off for the folder. Mac: try N6 and N7 again. | The Mac explains, before sending, that Windows has not turned on Agent requests, with the exact place on Windows. A request sent from a stale screen is refused by Windows; nothing runs. |
| N10 | Quit and reopen Pastey on Windows (or drop the network for 30 s). After it reconnects, look again from the Mac without any other step. | Sessions and the shared folder are back; the share survived the reconnect. |
| N11 | Windows: Stop sharing. Mac: look again. | "Windows hasn't shared any session-readable workspace with this Bridge…"; Run offers only the typed path or this device. |
| N12 | A second Bridge between the same devices: look there from the Mac. | Nothing from the first Bridge's shares appears. |

## Record

Copy this table per run and attach `tools/net-probe/results/*.jsonl` for part F.

| Field | Value |
|---|---|
| Date, commit, app versions (Mac / Windows) | |
| Mac and Windows OS versions | |
| Networks used (type, ISP or carrier; no IPs) | |
| Windows firewall choice | |
| Steps passed | |
| Steps failed (step, what happened, time) | |
| Steps not run, and why | |
| Remote session reading (I1–I7) | |
| Desktop UI (J1–J7) | |
| Crash diagnosis (K1–K6) | |
| Device details (L1–L5) | |
| Session continuation (M1–M9) | |
| Bridge workspaces (N1–N12) | |
| Throughput: C2 1 GiB each way, G1 | |
| Reconnect times: D1–D4 | |
| Net-probe summary (part F) | |
