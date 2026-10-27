# Mac ↔ Windows physical acceptance

One procedure for the physical evidence that automated tests cannot give: secure pairing, LAN Bridge operation, reconnection, representative Agent and file workflows, unattended control with Full Access, and Bridges across the internet. Each step says what to do, what must happen, and what to record.

Until a step is run on real devices and recorded, it is **untested**, whatever the automated suites say. Deterministic evidence is described in [development](../development.md).

## Scope and boundaries

- **Production paths:** pairing, LAN Bridge, overlay (Tailscale/WireGuard) reach, the internet carrier (part W), Agent and Transfer. These are exercised through the packaged app.
- **Raw path measurement:** the isolated `tools/net-probe` (part F) measures iroh paths between the two devices without Pastey. Use it to explain a part W result, not instead of part W.
- **Infrastructure:** n0's free public relays, under their development and hobby terms, for both the carrier's default and the probe. Nothing paid is required. Part W5 optionally uses your own `iroh-relay`.

## Prerequisites

- **Devices:** a Mac (Apple Silicon, current macOS) and a Windows 11 x64 PC.
- **Builds:** both run packaged builds of the same commit. On the Mac run `npm run build:checked`; on Windows run `npm run build:checked:windows` ([development](../development.md#setup-and-builds)). Record the commit and the version shown in each app.
- **Codex:** installed and signed in on the Windows Host for part E. Windows managed-execution setup is separate: [Windows managed execution](windows-managed-execution.md).
- **Networks:**
  - a shared Wi-Fi network;
  - a phone hotspot;
  - for C3, C5 and parts of F, a second fixed network (university, office, or a remote location).
  - for part W, two networks that are not the same LAN: home Wi-Fi and a phone hotspot at minimum; a university or office network if available.
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
| E6 | Full Access, briefly (part U covers it fully). On Windows, Bridge details → Full Access → Allow Full Access…, tick that you understand, Allow. Start a task from the Mac. | It runs without Accept. Revoke there: the next task waits for Accept again. |
| E7 | Re-pair this Bridge with a **different** device's code (or after D6), then repeat E1 from that device. | Earlier tasks, conversations, folder shares and Full Access from before the re-pair are gone. Running without Accept needs Full Access given to the new device. |

## U. Unattended control with Full Access (Mac controls Windows, nobody at Windows)

Run before part W when the internet tests are to be driven from the Mac alone. Prepare Windows as in [Unattended Windows readiness](#unattended-windows-readiness) below, at the desk, before leaving it.

| # | Do | Expect |
|---|---|---|
| U1 | Pair a new Bridge (B1). On Windows, look above the Bridge's tabs. | "Use this device from <Mac> when nobody is here?" with Review Full Access… and Not now. Not now hides it for this Bridge only. Bridge details → Full Access says Off, and every request waits for Accept. |
| U2 | Windows: Bridge details → Full Access → Allow Full Access…. Read the dialog. | It lists what the Mac may do (Codex in any folder, sessions, a terminal with Pastey's rights, not limited by folder rules), what it never does (control in the other direction, answer Codex's own prompts, raise its rights past UAC or privacy prompts, continue Claude Code), and what ends it. The fingerprint matches the Mac's Settings → This device. With Pastey started normally there is no administrator warning. Allow stays disabled until "I understand" is ticked. |
| U2a | Quit Pastey on Windows, start it with right-click → Run as administrator, and open the same dialog; then quit and start it normally again. | Started as administrator, the dialog warns in red that the terminal and Codex runs would have administrator rights, with no UAC prompt, and the tick says so. Started normally, the warning is gone. |
| U3 | Tick it and Allow. Open Settings → Permissions and Settings → Devices & trust on Windows. | Full Access: On. Permissions lists the Mac under Full Access with Revoke; Devices & trust lists it as trusted. |
| U4 | Direction. On the Mac, open Bridge details → Full Access. From Windows, start a Codex run on the Mac. | The Mac says Off: Windows got nothing. Windows's run waits for Accept on the Mac. |
| U5 | Leave Windows untouched from here on. On the Mac: Agents → Windows. Open Access. | The status line says "Connected · Codex ready · Full Access". Access shows Full Access as Ready, and Run Codex as starting at once. |
| U6 | Mac: Other places → A folder by path on Windows, type a test folder path and a small task, Run with Codex. | No Accept appears on Windows; the task runs and its reply reaches the Mac. If Codex asks to run a command or change a file, the request appears on the Mac with Approve once / Deny, never on Windows, and is never answered by itself. |
| U7 | Mac: Workspaces on Windows → + (Add a folder on Windows), type the full path of the folder used in part M (with a Codex thread in it). Then type `C:\Users\<you>`. | The folder appears marked Full Access with its sessions listed; on Windows, Bridge details → Full Access lists it. The home folder is refused as too broad to share, and no path is shown on the Mac. With every Codex client on Windows closed, continue one of its threads from the Mac: it completes without Accept and the thread on Windows has one more turn. Record whether a locked Windows session behaves the same. |
| U8 | Mac: Developer Mode → request a terminal on Windows. Run `whoami`, `whoami /groups \| findstr /i "Mandatory"`, `net session`, then `dir C:\` and `dir $env:USERPROFILE\Documents`. | It opens at once as PowerShell. `whoami` is your Windows user; the mandatory label is Medium (not High); `net session` is refused with access denied. Both folders list, although neither could be added as a workspace: folder rules do not limit the terminal. Windows shows "<Mac> has a terminal open on this device" with Close terminal. |
| U9 | Windows (one step at a desk, or via U8's terminal ending): Close terminal. | The Mac's terminal ends with "the other device closed it". |
| U10 | Restart Pastey on Windows (Quit from the tray, start it again); close its window (it stays in the tray); restart Windows and sign in, letting the Startup shortcut start Pastey; lock Windows (Win+L) and leave it locked. After each, without touching Windows further, repeat U6 and U8 from the Mac. | After a restart the Bridge reconnects by itself; record how long it took. Every time, the run and the terminal start without Accept. Record the locked-session result: it is the first evidence for it. |
| U11 | Across networks: repeat U6 and U8 with the Mac on the phone hotspot (part W setup). | Same as on the LAN; nothing waits for Windows. |
| U12 | Start a long Codex task from the Mac (for example "count slowly to 300, one number per line, with a short pause between") and open a Mac terminal. While both run, Windows: Bridge details → Full Access → Revoke. | The Mac's terminal closes (the other device closed it). The Mac's task shows "Stop requested: Full Access ended", not "Stopped"; on Windows, Codex stops (no Codex turn running in Task Manager after a few seconds). The next run from the Mac waits for Accept; the folder added in U7 is no longer offered. A task Windows's user had accepted earlier keeps running. |
| U13 | Allow Full Access again. Then on Windows: Settings → Devices & trust → revoke the Mac (or Forget it under Paired devices); trust or pair it again. | After revoking or forgetting, the next run waits for Accept, and trusting or pairing again does not bring Full Access back. Burn (part H) also ends it. |
| U14 | Only on a Windows install that had an unattended permission from an earlier build: after updating, start a run from the Mac in that folder. | It waits for Accept; the old permission was removed, not turned into Full Access. |

### Unattended Windows readiness

What the code does without anyone at Windows, and what Windows itself must allow. Checked in the source; only U10 shows it on a real PC.

- **Pastey keeps running and reconnecting.** Closing the window hides Pastey to the tray; only Quit ends it. The Bridge lifecycle re-checks every active, paired Bridge every 2 seconds plus the length of each try, and keeps retrying for as long as Pastey runs. "Disconnected" changes what is shown, not whether it retries. Each try looks at the remembered address, the LAN and, when Across the internet is on, the internet carrier at once. After a restart, Pastey restarts each Bridge's server, the carrier (if it was on) and this loop; Full Access is durable and needs nothing.
- **Nothing on the controlled side waits for its window.** Admission, the terminal, status and session answers run in Pastey's Rust process, so a hidden window or a locked screen does not hold them up. A locked session is expected to work but is untested.
- **Pastey must be running after a reboot.** Pastey has no background service and does not start at sign-in by itself. Add it once: press Win+R, run `shell:startup`, and put a shortcut to Pastey in that folder (start it normally, not as administrator). Without that, a reboot leaves Windows unreachable until someone starts Pastey.
- **Windows must stay awake and signed in.** Settings → System → Power & battery: when plugged in, never sleep (screen off is fine). Windows Update can restart the PC outside active hours and stop at the sign-in screen: set active hours to cover the test, or pause updates for its duration. Automatic sign-in after a restart is Windows's own setting and lowers the PC's physical security; this procedure does not require it.
- **Answer one-time prompts before leaving.** The first Pastey connection may show a Windows Firewall prompt, and Codex its own sign-in; run E0, U6 and U8 once at the desk.
- **Prompts no one can answer.** A UAC prompt, a Codex question set by Codex's own approval settings on Windows (it goes to the Mac), or a Pastey Accept for a device without Full Access will wait, then expire or stay pending.

## F. Raw iroh paths across networks (isolated probe, optional)

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

These bars describe the networks, not Pastey; part W is the test of the Bridge:
- C2 reaches direct in at least 8 of 10 runs within 10 s.
- 100% of digests intact.
- Relay-only runs report `relay` truthfully.
- Every R-cell outage recovers or is reported open.
- The unlisted key in M1 is rejected.

## G. Cross-network Bridge through an overlay address (optional)

| # | Do | Expect |
|---|---|---|
| G1 | Tailscale on both; the Mac on a phone hotspot, Windows at home. Mac: Create Bridge. Windows: Join with "another network" and the Mac's Tailscale name. | Pairs. A 256 MiB file transfers with matching hashes. |
| G2 | Set the remembered address on both. Switch the Mac's network, or quit and relaunch Pastey. | Reconnects through the overlay address. |

## W. Bridge across the internet (internet carrier)

Run on packaged builds of the same commit. Until W1–W14 are recorded, Pastey makes no claim that Bridges work across real networks. Keep each device's Settings → General → Across the internet line in view, and the Bridge's details → Reach it on another network line ("Across the internet: …"), which shows **In use: directly** or **In use: through a relay**.

Setup:
1. Both devices on the same Wi-Fi. Pair a new Bridge by code (part B). Send one text both ways.
2. On both: Settings → General → Across the internet → turn on **Connect across the internet** (leave the relay as n0's). On Windows, record what the firewall asks and what you allow.
3. Wait 10 s. In the Bridge's details on each device the line reads "Ready. Used when the other device is not on this network." (If it says the other device has not turned it on, wait for the next reconnect or send a text, then check again.)

| # | Do | Expect |
|---|---|---|
| W1 | Move the Mac to the phone hotspot (Windows stays on home Wi-Fi). Wait up to 60 s. | The Bridge shows Reconnecting, then Connected. Bridge details: "In use: directly" or "In use: through a relay"; record which and how long it took. |
| W2 | Send a text and the small file both ways; then a 256 MiB file Mac → Windows. | Text and small file arrive. The 256 MiB file arrives with a matching hash (`shasum -a 256` / `Get-FileHash`). Record the time and the path. Over a relay expect about 1 MiB/s; that is a limit, not a failure. |
| W3 | Agents: continue a shared Codex thread on Windows from the Mac (as M1), Accept on Windows. | Runs once; the reply arrives on the Mac; Windows's Codex shows exactly one new turn. |
| W4 | Start another continuation; while the Review waits on Windows, turn the hotspot off for 20 s, then on. Then Accept. | Nothing runs before Accept. After the network returns the Bridge reconnects (or stays Connected), and Accept runs it once. If Accept was not possible in time, it ends as not accepted; nothing runs twice. |
| W5 | Start a continuation, Accept it, and while it runs turn the hotspot off for 60 s, then on. | On the Mac the task ends as completed or as needing Reconcile. Reconcile shows the true outcome. Windows's Codex shows one turn for it, never two. |
| W6 | While Connected over the internet, switch the Mac from the hotspot to a different network (e.g. university or a second hotspot) without quitting Pastey. | Within about 60 s it is Connected again; a text goes both ways. Record whether the session stayed Connected throughout (path change only) or reconnected. |
| W7 | Quit and relaunch Pastey on the Mac (still on the other network). | Connected again within about 60 s with no code; record the time. |
| W8 | Sleep the Mac for 5 minutes on the other network, then wake it. | Connected within about 60 s of waking. |
| W9 | Restart Windows (Pastey starts at login, or start it). | Connected again after Pastey starts on Windows. |
| W10 | Network where UDP is blocked or the hotspot of a provider with strict NAT, if available (university Wi-Fi often qualifies). | Connected "through a relay", or not reachable across the internet. Never shows "directly" when the relay carries it. Record which. |
| W11 | On Windows only, turn Connect across the internet off. | Within about 30 s the Mac shows the Bridge not reachable (or reconnecting); nothing is sent. Turn it back on: Connected again within about 60 s. |
| W12 | Bring both back to the same Wi-Fi. Then quit and relaunch Pastey on the Mac. | Stays Connected. Before the relaunch the session may still run over the carrier ("In use: directly", now on the local network). After the relaunch it reconnects over the LAN and the line reads "Ready…". Send a 1 GiB file and compare with C2's time: within 10%. |
| W13 | With the Mac on the hotspot and Connected over the internet, Burn the Bridge on Windows. | The Mac shows the peer departed (or not reachable) and does not reconnect, across the internet or the LAN. A new Bridge needs a new pairing on the same network. |
| W14 | Optional: run your own `iroh-relay` (see iroh's documentation), set its `https://` address on both under Relay, and block n0's relays (or the direct path) to force the relay. Repeat W1–W2. | Connected through your relay; nothing contacts n0 (check the relay's log for both devices). |

Record for each of W1–W14: networks (type and provider; no IPs), path shown, times, hashes, and the Windows firewall choice.

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
| I1 | On the Mac, Bridge → Agents, Agents on Windows, before any permission. | "Windows has not allowed this device to read its sessions…", with where to allow it. No session is listed. Access on Windows → Read sessions now says Needs action, with the steps on Windows. |
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
| K5 | Reproduce E1 with this build, once with Accept on Windows and once under Full Access. | Both complete. If it crashes, K1–K4 now have the cause. On the Mac the task shows the indeterminate notice; Reconcile, then Stop. Nothing is run again automatically. |
| K6 | Start a task from the Mac, switch to another Bridge and back (or reload with ⌘R). | The running task card is still there, names Windows, and Stop still works. |

## L. Device details and diagnostics

| # | Do | Expect |
|---|---|---|
| L1 | Click the device name in a Bridge header, or a device in Bridge details. | Device Details opens with Overview, Connection, Performance, Agents, Trust & permissions and Advanced. |
| L2 | Performance → Test connection. | About a second on a fast connection (longer on a slow one: it waits for what it sent), then a measured average and peak MB/s, latency, quality and "Measured just now", labelled as Pastey's pipeline over this Bridge in memory. The peak is never below the average. Record the numbers. |
| L3 | Advanced → Run managed self-check. | Managed readiness and the E2E result (PASS, BLOCKED or FAIL), as before under Bridge Devices → Check. |
| L4 | Settings → This device: Refresh, Full check, Run local test (each mode once). | Device facts, runtimes with versions, and a local-only measurement labelled "no network". |
| L5 | Settings → General → Receiving folder → Change…, receive a file, then Use default. Turn Save received files off and receive a file. | The file lands in the chosen folder; with saving off it is not put in the receiving folder. |

## M. Native session continuation (Codex threads, Claude Code sessions)

Prerequisites: Codex 0.163 or later on both devices (CLI and, if installed, the ChatGPT desktop app); on Windows, Python 3 for the probe; Windows shares the Demo folder with this Bridge with Sessions and Agent requests on (I2, then turn on Agent requests).

| # | Do | Expect / record |
|---|---|---|
| M1 | On Windows: `python -I tools\codex-session-probe\writer_lock_probe.py (Get-Command codex).Source`, once with the ChatGPT app open on a Codex thread and once with `--cross-process`. | `b_resume_while_a_holds` and `resume_of_thread_held_elsewhere` say "already has an active writer"; after A exits, B resumes. This is the Windows proof of the single-writer lock the remote path relies on. Record the JSON (ids are shortened). |
| M2 | Mac, local: Agents on This device → add a folder with a Codex thread you made in the ChatGPT app (the list fills itself) → open it. With the thread still open in the app, read the view. | Row chip "Open elsewhere"; the view says "You can't continue this session now" because it is open in another Codex client on this device; Open anywhere? says "Open in the Codex app or a terminal"; Evidence → source "Codex thread writer lock". |
| M3 | Close that thread in the app (open another), Look again, open it, Continue with "Reply with the word ok". | The form is replaced, in the session's view, by the run: Started here → Running → Result, Completed with the reply. The view then reads the session again (one more prompt). In the ChatGPT app, the thread shows the new turn. The thread id is unchanged. |
| M4 | Repeat M3 but, before pressing Continue, add a turn to the thread in the app. | "This session changed since you looked at it, so nothing ran." Nothing new appears in the thread. |
| M5 | Mac → Windows: Agents on Windows → Demo → open a Codex thread that the app on Windows does not have open → Continue. On Windows, Accept. | The Windows request banner says "continuing an existing thread" and "in your shared folder Demo". On the Mac, in the session's view: Sent to Windows → Waiting for Accept → Running → Result. The reply returns there; the view then shows the new turn. |
| M6 | M5 again, but Deny on Windows; then once with the thread open in the Windows app, Accept. | Deny: "Declined on Windows · nothing ran", the Accept stage marked Not accepted. Open in the app: "Did not start · nothing ran", the Host's reason ("open in another Codex client"), and Next: close it in the Codex app or terminal on Windows, then look again. |
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
| N6 | Mac: Agents on Windows → PasteySessionTest → New conversation, "List the files", send. On Windows, Accept. | Runs in `PasteySessionTest` on Windows without typing a path; the reply returns to the Mac. |
| N7 | Mac: PasteySessionTest's sessions (listed by themselves) → open the Codex thread → send "Reply with the word ok". | Windows shows the Review ("continuing an existing thread … in your shared folder PasteySessionTest"). The Mac shows "Waiting for someone on Windows to Accept" in the session's view. |
| N8 | Windows: Accept. | Codex continues in the same thread (the ChatGPT app shows the new turn when you open the thread); the Mac's session view completes with the reply and then shows the thread again with one more turn. |
| N9 | Windows: turn Agent requests off for the folder. Mac: try N6 and N7 again. | The Mac explains, before sending, that Windows has not turned on Agent requests, with the exact place on Windows. A request sent from a stale screen is refused by Windows; nothing runs. |
| N10 | Quit and reopen Pastey on Windows (or drop the network for 30 s). After it reconnects, look again from the Mac without any other step. | Sessions and the shared folder are back; the share survived the reconnect. |
| N11 | Windows: Stop sharing. Mac: look again. | "Windows hasn't shared any session-readable workspace with this Bridge…"; Run offers only the typed path or this device. |
| N12 | A second Bridge between the same devices: look there from the Mac. | Nothing from the first Bridge's shares appears. |

## P. Sessions outlive their clients; who has a thread open

What a user sees about a session versus the programs that had it open. Prerequisites: N1–N5 done; on Windows `PasteySessionTest` has one Codex thread made in the ChatGPT desktop app.

| # | Do | Expect / record |
|---|---|---|
| P1 | Windows: keep the ChatGPT app open (any thread). Mac: Agents on Windows → PasteySessionTestssion. | The thread is listed. Row chip "Can try"; its view says "You can try: Codex decides when it starts" and Open anywhere? says Codex is running on Windows and Pastey cannot tell which threads it has open. Evidence → source "process list". Nothing on Windows changes. |
| P2 | Windows: quit every Codex client (the ChatGPT app, `codex` terminals; check Task Manager has no `codex.exe`). Mac: Look again. | The thread is still listed: it is saved history, not a process. Chip "Can continue"; Open anywhere? "No Codex app or terminal is running on Windows, so nothing has it open"; Evidence says this is Windows's list of running programs, device-wide. |
| P3 | Mac: continue it ("Reply with the number 73921 only"); Windows: Accept. | Completed with 73921 in the session's view, which then shows 2 turns. Look again: still "Can continue". The ChatGPT app, opened again, shows the new turn in the same thread. |
| P4 | Mac: PasteySessionTest → New conversation ("List the files"), Accept on Windows. Then refresh PasteySessionTest's sessions. | The thread Pastey used is "Started by Pastey", "Open in Pastey, idle", chip "Can continue"; Open anywhere? says Pastey's own conversation has it open and continuing closes that first. Other threads in the folder are no longer "Unknown" merely because Pastey has its own session there; record what they show (a Codex that Pastey starts through a launcher such as npm's `codex.cmd` runs as a grandchild, which the process list cannot tell from another client). |
| P5 | Open that thread from the list and send a message; Accept on Windows. Then, in the New conversation from P4, reply once more. | The continuation completes in the same thread. The reply in P4's conversation continues that same conversation ("Continues this conversation" under the composer). A fresh New conversation always starts a new thread. Nothing reports "open in another Codex client". |
| P6 | While a New conversation runs in PasteySessionTest, open the Pastey thread from the Mac. | "Running", "A Pastey task is running a turn in it now"; Continue is unavailable with that reason. |

On a Mac (local), P1 instead shows the per-thread writer lock: only the thread the app has open is "Open elsewhere"; the others are "Can continue".

## Q. Agents as conversations (layout and flow)

Prerequisites: N1–N5. Run at the default window size (about 760 px wide) and once wider than about 1,100 px.

| # | Do | Expect / record |
|---|---|---|
| Q1 | Mac: Bridge → Agents. | Wide: navigator (Agents on Windows, connection and Codex readiness, Shared workspaces with "Sessions" / "Agents" per folder, Other places) beside a New conversation. Default size: the navigator alone. No Windows path anywhere. |
| Q2 | Select PasteySessionTest. | It opens to New conversation and its sessions, listed by themselves, each with "Can continue", "Can try", "Open elsewhere", "Running" or "Only on that device". A workspace shared without Agents offers no New conversation and says why. |
| Q3 | Open a Codex session. | Its history as a conversation (prompts right, Codex left, "its own account, not verified"), events as quiet lines; with excerpts off, "Windows shares this session's status only". At default size, a back button returns to the navigator. Details (ⓘ) shows state, who has it open and how Pastey knows, ids and evidence. |
| Q4 | Send "Reply with the word ok"; Accept on Windows. | Your message on the right, then the turn on the left: "Waiting for someone on Windows to Accept", Running, then the reply. No text appears before the reply arrives. The session is then read again; nothing shows twice. |
| Q5 | With the thread open in the Codex app on Windows, send again. | The turn ends "Did not start · nothing ran" with Windows's reason and Next: close it there. The composer says "You can try: Codex decides when it starts" beforehand. |
| Q6 | New conversation: "List the files"; Accept; then reply "and count them"; Accept. | The first turn starts a new thread (Task details → Conversation: new); the reply continues it (continued). The conversation is listed in PasteySessionTest as "your new conversation, from this device". Starting another New conversation begins a new thread again. |
| Q7 | While Q6 runs, open another session. | The navigator shows a banner with the running turn; clicking it returns to the conversation. The other session's composer says Codex is working on another task in this Bridge. |

## O. Control channel over a long session

What failed on real devices before this build: after about 64 Codex availability checks (each Run or Continue checks first; the Agents tab checks every 15 s), every later check failed with "Room control inbox is full" until Pastey restarted. Use both devices on this build. Record the Control channel facts (Device Details → Advanced → Check control channel, on each device) at the start and the end.

| # | Do | Expect / record |
|---|---|---|
| O1 | Leave the Mac's Agents tab open on Windows for 30 minutes (about 120 automatic checks), with Pastey on Windows running throughout. | "Codex is available on Windows" throughout. On both devices: "Accepting control messages. Nothing is backed up.", Kept for display 0 (or a few Bridge Plan events after Find file), no refusals. |
| O2 | Then, without restarting either side, N6–N8 (run in a shared folder, continue the thread, Accept on Windows). | Each completes; the reply returns to the Mac. |
| O3 | On the Mac, click Check again (Agents → New conversation) 30 times as fast as you can. | Most answer at once; some may take a few seconds. If any fails, it says "rate limit was reached … refused this before acting on it". Windows's Control channel shows capability checks refused for their rate budget, then, within a minute and with no action, "Accepting control messages". |
| O4 | Send 9 continuations from the Mac without answering on Windows. | Windows shows 8 Reviews. The ninth's card on the Mac says "not delivered · nothing ran" with the reason; Windows's Control channel shows 8 of 8 waiting. Deny one on Windows, continue again: it waits for Accept. |
| O5 | While a continuation waits for Accept on Windows, turn Agent requests off for the folder (Bridge details → Workspaces), then Accept. | Accept is refused; nothing runs (no new turn in the thread). The Mac card ends "access ended before it started". |
| O6 | Quit and reopen Pastey on Windows, then repeat O1 for 5 minutes and O2 once. | Same as O1 and O2; no "inbox is full" ever. |
| O7 | On Windows, `Get-Content "$env:LOCALAPPDATA\pastey\logs\pastey.log" -Tail 200 \| Select-String "room-control"`. | Only `backpressure` / `backpressure_ended` lines from O3, with a class and counts; no prompts, paths or ids beyond 8 characters of the Bridge id. |

## X. WAN retest of the 27 October 2026 findings

What was seen on beta.2 with the Mac on an iPhone hotspot and Windows at home, over a direct path: Test connection reported 0.2 MB/s average but a 14.1 MB/s peak; Agents said "Current remote Host session is unavailable" while the Bridge showed Connected; the conversation said Windows must accept although Windows had given the Mac Full Access; and after a while Developer Mode refused with "Developer Mode UI authority is unavailable" until Pastey restarted. Run on packaged builds of the fixing commit, in the same setup (part W's W1).

| # | Do | Expect / record |
|---|---|---|
| X1 | Device Details → Performance → Test connection, three times, about a minute apart. Then on the Mac: `grep -E "link_benchmark_(summary\|path)" ~/Library/"Application Support"/pastey/logs/pastey.log \| tail -6`. | Peak ≥ average, and close to it when every chunk took the whole run. Record average, peak, latency, duration, and from the log `chunks`, `first_ack_ms`, `longest_chunk_ms`, `path=` before and after, `rtt_ms`, `lost_packets`, `congestion_events`, `black_holes`. A congested uplink usually shows `rtt_ms` well above the idle latency and new congestion events; without them, look at `path=` (a path change) and `black_holes` (an MTU problem). The log names the Bridge by id only. |
| X2 | Same, with the relay forced (W10 or W14). | The log shows `path=Relayed`; expect about 1 MiB/s (part F). |
| X3 | Raw path: net-probe cell C2 from the Mac on the hotspot, `--bulk-mib 16 --http` (part F). | Bulk MiB/s of the raw iroh path on the same networks. If it is also about 0.2 MB/s, the hotspot's uplink is the limit and Poor is right. |
| X4 | Keep Agents open on Windows for 10 minutes with the Mac on the hotspot; turn the hotspot off for 20 s once, then on. | After the Bridge is Connected again, "Codex is available on Windows" returns within about 20 s without Check again. "Could not check … session is unavailable" may show during the outage only. |
| X5 | With Full Access given on Windows, open a new conversation in a Windows folder and continue it. | The conversation says it starts on Windows at once (Full Access), not that Windows must accept. Its progress shows "Checking there" then Running, never "Waiting for Accept". Revoke Full Access on Windows, wait 15 s and click into the Mac window: the next run says it waits for Accept, and it does. |
| X6 | Enter Developer Mode on the Mac, open a terminal on Windows, wait more than 30 minutes (or turn the hotspot off until the Bridge reconnects), end the terminal. | Developer Mode says it ended and offers Enter Developer Mode; entering again and requesting a terminal works without restarting Pastey. A terminal still open when the 30 minutes pass keeps working until its own 30 minutes end, and is not reopened. Without Full Access, Windows still has to Accept the new request. |

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
| Sessions outlive clients; holds (P1–P6) | |
| Agents as conversations (Q1–Q7) | |
| Bridge workspaces (N1–N12) | |
| Full Access, unattended (U1–U14; lock-screen result from U7) | |
| Control channel (O1–O7) | |
| Throughput: C2 1 GiB each way, G1 | |
| Reconnect times: D1–D4 | |
| Internet carrier (W1–W14): networks, path shown, times |
| WAN retest (X1–X6): benchmark numbers and log fields, recovery times |
| Net-probe summary (part F) | |
