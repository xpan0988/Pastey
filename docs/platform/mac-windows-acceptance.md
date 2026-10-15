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
| A2 | Settings → this device's identity on each. | A 20-character fingerprint; note it for B and E. |

## B. Secure pairing (both devices on the same Wi-Fi)

| # | Do | Expect |
|---|---|---|
| B1 | Mac: New Bridge → Create Bridge. Windows: New Bridge → enter the Mac's code → Join. | Both show the Bridge as Connected; the network panel says **Paired device** on both. |
| B2 | On a new Mac code, type a wrong code on Windows three times, then the right one. | Wrong codes: "did not answer" each time, and no Bridge appears on Windows. The right code then pairs. |
| B3 | After B1, enter the same code again on Windows, or on a third device if available. | Refused or no answer. A code admits one device. |
| B4 | Nearby flow. Windows: New Bridge → Nearby → Join on the Mac. Mac: Accept. | The Mac shows "Enter NNNN NNNN on <Windows>". Windows shows an "accepted" card asking for the code. Typing it pairs. Repeat once with **Reject**; Windows says rejected. |
| B5 | Explicit re-pair. On the Mac's Bridge from B1: network panel → Pair another device. On Windows, enter that code under **New Bridge**. | Refused: "This device already has that Bridge". Then, on Windows, open that Bridge's network panel, type the code in **Pair with code**, and pairing succeeds. |
| B6 | Legacy upgrade, if a pre-pairing build with an existing Bridge is available. Before upgrading, share a folder with that Bridge. | After upgrading both: the Bridge shows **Pairing required**, never reconnects on its own, and its history is visible. Re-pair it as in B5 (Show new code on one side, Pair with code on the other). Afterwards the folder share is gone and must be shared again; the history is still there. |

## C. LAN Bridge operation

| # | Do | Expect |
|---|---|---|
| C1 | Send text both ways. | Arrives once, exact. |
| C2 | Send the 1 GiB file Mac → Windows, then Windows → Mac. | Completes, and the SHA-256 matches. Record throughput from the transfer card. |
| C3 | Paste an image; send the small file to the Bridge. | Both arrive; the hashes match. |
| C4 | Start the 1 GiB send. Cancel it on the sender at about 30%. Repeat and cancel on the receiver. | Both sides show cancelled or interrupted; no partial file appears in the Inbox. |
| C5 | On Windows, share the test folder with this Bridge. On the Mac, Find file by a name inside it, and receive one file. | Only the chosen file arrives, hash intact. The device that ran the search changes nothing on Windows. |
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
| D6 | On Windows only: Settings → Unattended Codex access → Replace identity key…, then restart Pastey on Windows. | The Bridge stays disconnected: the Mac refuses the new key on reconnect. Re-pair: on the Mac, network panel → Pair another device; on Windows, that Bridge's network panel → Pair with code. Then it is Connected. |

## E. Agent workflows (Codex on Windows, requested from the Mac)

| # | Do | Expect |
|---|---|---|
| E1 | From the Mac, start a small Codex task on Windows. | Windows shows the executor Review. Accept runs it; the reply returns to the Mac card. |
| E2 | Repeat and choose Deny. | Nothing runs; the Mac shows that it did not start. |
| E3 | Workspace movement: propose with a small folder, approve on the Mac, Accept on Windows. | The workspace lands, Codex runs, the result returns and applies, and the Mac's original folder is unchanged until it is applied. |
| E4 | Cancel a running task from the Mac. | Windows stops it, and both cards show cancelled. |
| E5 | Turn Mac Wi-Fi off during a task for 20 s, then reconnect. | Reconcile shows the true outcome. Nothing runs twice. |
| E6 | Unattended access. On Windows, compare the Mac's fingerprint (A2), trust it, and grant one folder. Start a task from the Mac. | It runs without Accept. Revoke the grant: the next task waits for Accept again. |
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
| Throughput: C2 1 GiB each way, G1 | |
| Reconnect times: D1–D4 | |
| Net-probe summary (part F) | |
