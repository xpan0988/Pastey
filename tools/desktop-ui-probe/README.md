# desktop-ui-probe

Standalone development probe using public macOS Accessibility APIs. Pastey does not use desktop UI automation for Native Agent execution or session discovery.

## Tools

Build on macOS:

```bash
cd tools/desktop-ui-probe && mkdir -p build && for t in probe send; do swiftc -O Shared.swift $t/main.swift -o build/$t; done
```

The process that runs them needs Accessibility permission. Grant it to the process that runs the tool in System Settings → Privacy & Security → Accessibility.

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
