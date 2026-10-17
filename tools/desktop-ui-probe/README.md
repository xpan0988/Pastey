# desktop-ui-probe: Desktop UI continuity PoC (research)

Research only. It is not part of the Pastey app, does not touch the Native App Server adapter (D1), Bridge or Burn, and is not integrated anywhere.

The question: can Pastey observe and use the user's existing ChatGPT / Codex desktop conversations through the desktop UI the user already has, instead of attaching to the app's private app-server (see `tools/codex-session-probe`)?

Everything here uses only the public macOS Accessibility API (AXUIElement), plus one Return key event. Nothing reads or changes the app's database, private IPC, files or credentials, or answers an approval.

## Tools

Build on macOS:

```bash
cd tools/desktop-ui-probe && for t in probe send; do swiftc -O Shared.swift $t/main.swift -o build/$t; done
```

The process that runs them needs Accessibility permission. In this PoC that was the Claude app, granted by the user and revocable in System Settings → Privacy & Security → Accessibility.

- `probe summary|watch` (read only): finds the app's window and reports what it can identify: the conversation list, the open thread, the message area, the composer and its buttons, run state and approval controls. `watch` samples once a second. Text is redacted to its length and a hash, unless `--reveal` is given for a test conversation.
- `send` (one guarded write, only for a conversation the user authorized):
  - **Before typing**, it checks the exact thread title, that no turn is running, that the composer is empty (or already holds exactly this message), that no approval control is shown, and that the nonce is not already in the thread.
  - **Typing**: it sets the message and reads it back exactly.
  - **Submitting**: it presses Send if the button lists `AXPress`. Otherwise it waits for the user to bring the app to the front, checks focus and text again, and posts one Return key.
  - **No retry**: it never sends twice. A send it cannot confirm is reported as unknown.
  - **Reply**: it waits for the turn to end and reads the reply.
  - **Never** presses an approval control.
  - A lock file allows one sender at a time. `--dry-run` stops before typing.
- `windows-uia-probe.ps1`: the Windows UI Automation counterpart of `probe summary`. **Unverified:** written on a Mac and not yet run on Windows.

`out*/` and `build/` are local and never committed: logs from a revealed run contain conversation text.

## Results, 2026-10-16/17, MacBook (macOS 26.5.2), ChatGPT desktop app (bundle `com.openai.codex`)

All on this real Mac, with the real app.

### Read only

| Question | Result |
|---|---|
| Find the window | Yes, by bundle id. One standard window; a full read of about 900 elements takes about 240 ms. |
| Tree available | Yes. Electron's `AXManualAccessibility` is answered "unsupported" (-25205), and the tree is exposed anyway. |
| Conversation list | Yes: sidebar buttons with class `sidebar-item`. Codex mode showed 37 (grouped by project); Chat mode showed 72. No selected state is exposed. |
| Current thread | Yes: the web area's title is the thread title. In Chat mode the window title equals it; in Codex mode the window title is the app's name. A title-hash match against `thread/list` found exactly one Codex thread. |
| Message area | Yes: the element with class `thread-scroll-container`. Its first child is the message list, a notice line sits outside it, and the composer comes last. Each turn starts with a screen-reader heading ("你说：", "ChatGPT 说：", the same in Codex mode). |
| Composer | Yes: a `ProseMirror` text area. When empty, its value is the placeholder plus a newline and its only child has class `placeholder`; no placeholder attribute is exposed. |
| Send button | Yes: the composer form's "发送" button, disabled while the composer is empty. |
| Run state | Only through the button that replaces Send while running ("停止"). No busy, progress or `AXElementBusy` signal exists. The label is localized. |
| Message content | Readable, but **only what the app has rendered**. A 12-turn Codex thread showed its last 2 user turns. The tree is not the history. |
| Window in background | Reading works unchanged (`front=false`), including live updates during a running turn. |
| Live vs persisted | The UI showed a running turn while the separate app-server's copy of that thread said its last turn was interrupted. The UI sees live state the persisted history does not. |
| Mode and update stability | Chat and Codex mode share every anchor above. Hashed CSS module names (`_yl8z4_` → `_etoz9_`) changed between two captures a day apart; the PoC uses only unhashed names. |
| Lock screen, minimized window, scrolling to load older messages | **Not tested.** |

### Write (test conversation "Pastey UI PoC", Chat mode, authorized by the user)

| Step | Result |
|---|---|
| Typing | `AXSelectedText` was accepted with no effect. Setting `AXValue` worked: the composer read back the exact message, and the app enabled Send, so its own state took the input. |
| Pressing Send through Accessibility | **Not possible.** The button lists only `AXShowMenu` and `AXScrollToVisible`. `AXPress` returns success and does nothing. Sidebar items are the same, so **switching conversations through Accessibility is not possible either**. |
| Bringing the app forward | **Refused by macOS** for a background process (`AXFrontmost`). The user has to bring the window forward. |
| Submitting | With the app in front, and focus and text re-checked, one Return key posted to the app sent the message. The nonce appeared in the thread 235 ms later and the composer cleared. |
| Status and reply | Running (Stop shown), then ended after 5 s. The reply read back as exactly `pastey-ui-poc-ok ui-134edc72`. |
| Duplicate protection | The failed `AXPress` attempt was detected as unconfirmed and not retried. A repeat with the same nonce was refused. A second sender was refused by the lock. |
| Approvals | None appeared. Approval controls are recognized only by label words, and are never pressed. |

Two bugs were found and fixed during the PoC:
- The placeholder was read as a draft.
- The first reply extractor anchored on the reply itself, because the reply quoted the nonce.

## Judgement: not worth integrating as a product adapter now

- **Reading is real but partial.** Only the rendered part of a thread is visible. Run state depends on a localized button label. There is no reliable status for approvals, errors or completion beyond that.
- **Writing needs the user present.** Buttons expose no press action, and macOS will not let a background process bring the app forward. Submitting therefore means a keystroke into the frontmost app, after the user brings it forward. That can never be done for a remote Bridge request without someone at the Mac. Doing it unattended would mean taking the user's foreground and keyboard, which is the fragile control this PoC was told not to build.
- **Navigation is not possible.** Switching conversations, scrolling history and stopping a turn would need clicks.
- **The authority would be new.** Desktop UI control acts as the local user inside their signed-in app. It would need its own authorization separate from Bridge trust, and must never be reachable from a peer.

The App Server adapter (D1) stays the supported path for running Codex. The Desktop UI route is useful at most as a local, read-only indicator of what the desktop app shows now, for example "the desktop is running a turn in this thread". That is the only part I would consider exposing, for example as a read-only local MCP tool. Writing through the UI should stay a manual, user-present action.

## Windows: an independent verification plan (not run)

On a Windows 11 PC with the ChatGPT desktop app, a test conversation open, and no other app's credentials involved:

1. **Read only:**
   ```bash
   powershell -ExecutionPolicy Bypass -File .\tools\desktop-ui-probe\windows-uia-probe.ps1
   ```
   Record `%TEMP%\pastey-desktop-uia\summary.json` after reviewing it. Then check:
   - whether Chromium exposes DOM classes as UI Automation `ClassName` (`thread-scroll-container`, `ProseMirror`, `placeholder`, `sidebar-item`);
   - whether the composer offers the Value pattern;
   - whether "发送" / "Send" offers the **Invoke** pattern. Unlike macOS's missing `AXPress`, this would allow a press without a keystroke.
2. **Boundaries:** repeat with the window in the background, minimized, and with the session locked (Win+L). Record whether the tree, the composer and the run state stay readable.
3. **Write, only after explicit authorization for one test conversation and one message:**
   - Port `send`'s guards as they are: exact title, idle, empty composer, no approval control, unused nonce, a lock, and no retry.
   - Prefer Invoke on the Send button. Use a keystroke only with the window in front and focus re-checked.
   - Windows also restricts `SetForegroundWindow` for background processes, so expect the same "user brings it forward" step.

Until these steps are run on a Windows PC, nothing about Windows is verified. A cross-compile or a script that parses proves nothing here.
