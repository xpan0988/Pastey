# Agent conversation UX — audit and refinement

Status: audit and implemented refinement, 2026-10-31, on `main` after 2.0.0 (`f351879`). The current behaviour is described in [UX and information architecture](../ux-information-architecture.md); this page records why it changed and what was left.

Evidence levels used below:
- **Observed**: seen in Pastey's code, or driven in its renderer (Chromium, through a temporary harness that mounted the real `Composer`, `ConversationThread`, `PastTurnView` and `TaskTurn`) for this report.
- **Documented**: from a vendor's own documentation, or a browser vendor's or a standards body's.
- **Third-party**: guides, cheat sheets and forum posts. Weighted low.
- **Inference**: reasoned, not verified.

The reference applications (Codex Desktop and CLI, Claude Code and Claude Desktop, ChatGPT Desktop) were **not** run or inspected for this report. What is said about them comes from documentation and third-party sources, and is labelled.

## 1. Audit: Pastey against the reference applications

| Dimension | Pastey before (Observed) | Reference practice | Level |
|---|---|---|---|
| Reply formatting | Agent replies shown with `white-space: pre-wrap`: `**bold**`, `#`, fences visible as syntax | Chat and coding Agents render Markdown with code blocks and copy | Inference (universal, not checked per app here) |
| Untrusted content | Replies escaped plain text; no links, no HTML (safe, but unreadable) | — | — |
| Enter / newline | `Enter && !shiftKey && !isComposing` sends | Claude Code: Enter submits; Shift+Enter, Option+Enter, `\`+Enter or Ctrl+J add a line; `chat:submit` / `chat:newline` rebindable | Documented ([Claude Code interactive mode](https://code.claude.com/docs/en/interactive-mode)) |
| | | Codex CLI: Enter submits, Ctrl+J newline, Esc interrupts | Third-party ([linuru](https://linuru.com/codex-cli/)) |
| | | ChatGPT macOS: Return sends, Shift/⌘+Return newline; users ask for a toggle | Third-party ([OpenAI community](https://community.openai.com/t/chatgpt-app-allow-toggle-newlines-and-cmd-ctrl-return-enter-for-submission/1253379)) |
| IME | `isComposing` only | WebKit can deliver `compositionend` before the confirming keydown, which then has `isComposing === false` and `keyCode === 229` (WebKit bug 165004); check both, with a ~50 ms `event.timeStamp` window as a fallback | Documented ([Chrome IME-safe Enter guidance](https://developer.chrome.google.cn/docs/modern-web-guidance/guides/forms/ime-safe-enter-submit), [WebKit 165004](https://bugs.webkit.org/show_bug.cgi?id=165004)) |
| Duplicate send | Guard on a React state flag read from a stale closure; held Enter repeats | — | — |
| Pending send | Agent composers `disabled` while sending and while a turn ran: focus lost, next message could not be drafted | Chat apps keep the field usable and block only sending | Inference |
| Drafts | A session's draft lost on navigating to another session (component remounts) | — | — |
| Scrolling | Every update jumped to the bottom, even while reading history | "Scroll to newest" affordance while reading back | Inference |
| Shortcuts | Hint line replaced by any blocker text, so shortcuts were rarely visible | Apple: prefer ⌘ as the modifier, don't repurpose standard shortcuts | Documented ([HIG Keyboards](https://developer.apple.com/design/human-interface-guidelines/keyboards)) |
| Claim vs fact | Every reply labelled "its own account, not verified"; turn stages from Pastey's own lifecycle | — Pastey is ahead here | Observed |
| No fake progress | Turn shows Sent → Accepted → Running → Result, never partial text | — Pastey is right not to imitate streaming | Observed |

## 2. Gaps: evidence, impact, severity

| # | Gap | User impact | Severity | Done |
|---|---|---|---|---|
| G1 | Markdown shown as syntax | Long Agent replies (lists, code, tables) are hard to read; code cannot be copied cleanly | High | Yes |
| G2 | IME Enter sends on macOS (WebKit order) | Chinese/Japanese/Korean users send half-written messages to an Agent on another device, which then runs | High | Yes |
| G3 | Double/held Enter can send twice | Two remote runs for one intention | High | Yes |
| G4 | Composer disabled while sending / running | Focus lost; cannot prepare the next message | Medium | Yes |
| G5 | Session draft lost on navigation | Retyping; or worse, nothing | Medium | Yes |
| G6 | Thread jumps while reading | Cannot read history while a turn updates | Medium | Yes |
| G7 | No copy for a reply or code block | Selecting rendered text is lossy | Medium | Yes |
| G8 | No Enter-to-newline preference | Users who write long prompts send early | Low | Yes |
| G9 | Links cannot be opened | Must copy the address | Low | Deliberately not (§3) |
| G10 | Narrow bubble (≤ 520 px) for long replies | Code wraps or scrolls in a cramped box | Low | Yes |

## 3. Design principles: adopt, adapt, reject

**Adopt**
- Enter sends, Shift+Enter adds a line, ⌘/Ctrl+Enter always sends; a preference to swap (Claude Code makes these rebindable; ChatGPT users ask for it).
- Markdown with GFM, code blocks with language label and Copy, Copy for the whole reply as the original text.
- Keep the composer usable while work is in flight; block only the send.

**Adapt**
- **Links.** Local apps open links in the browser. In Pastey a reply often comes from another device's Agent, whose text Pastey cannot vouch for, and the webview has no opener permission (only `dialog:default`). A link therefore never navigates: it shows its real host when its text says otherwise (`docs (evil.example)`) and copies the address. Opening in the browser needs a new, validated Host command and is deferred (§6).
- **Images** are placeholders: fetching one would tell a third party when and where a reply was read.
- **Highlighting** only for a fence that names a known language; no auto-detection (cost, and misleading colours on terminal output).
- **Scroll following**: follow unless the user scrolled up; a send of one's own always follows.

**Reject**
- **Esc to stop** (Codex CLI). Stopping a turn on another device has consequences Pastey shows explicitly (the stop may not take, the outcome can be ambiguous); it stays a labelled button.
- **Streaming or a "thinking" placeholder.** Pastey receives a reply only when a turn ends; showing anything else would invent progress.
- **Rendering everything as Markdown.** Prompts, Share text (often clipboard content), terminal output, diagnostics and native request content are shown verbatim.
- **Persisting drafts to disk.** Prompts may be sensitive; drafts live in memory for the session of the app.

None of this changes Layer 4/5 authority, task lifecycle, recovery or execution contracts: the composer still sends only when the existing `canSend` allows it, and the same blockers apply.

## 4. Implemented

| File | Change |
|---|---|
| `src/features/workspace/AgentMarkdown.tsx` (new) | `react-markdown` + `remark-gfm`; raw HTML stays text (no `rehype-raw`); `urlTransform` admits only http, https, mailto; `a` → a button that copies the address and shows the host; `img` → placeholder; `pre` → code block with language, Copy code, `lowlight` highlighting for 21 named grammars (aliases included), none above 20 000 characters; headings rendered as `role="heading"` at level+2 under the conversation's h2; tables scroll horizontally; memoized. `CopyButton` and `copyText` (Host clipboard in the app, browser clipboard in preview). |
| `src/lib/composerKeys.ts` (new) | Pure `composerKeyAction` (Enter, Shift, Alt, ⌘/Ctrl, repeat, `isComposing`, `keyCode 229`, own composition flag, 50 ms `timeStamp` window either side of `compositionend`); Enter preference in renderer storage (default "adds a line" on a touch-first device); platform-named shortcut hint. |
| `src/features/workspace/Conversation.tsx` | `ComposerInput` (composition tracking, auto-grow to `min(45vh, 320px)`, read-only while sending so focus and text stay, `aria-keyshortcuts`), `SendButton` (shortcut in its tooltip), `createSendGuard` / `useSendGuard` (one send until the sender's promise settles), `Composer` keeps focus after a click send; `ConversationThread` follows only when at the end, offers **Newest**, follows after one's own send. |
| `AgentTaskTracker.tsx`, `SessionDetail.tsx` | Replies, earlier turns and session Agent excerpts use `AgentMarkdown` with Copy reply; prompts stay verbatim. Session composer: in-memory draft per device and session; disabled only when the session cannot be continued; drafting allowed while a turn runs. |
| `NativeAgentPanel.tsx` | `run()` returns the start promise to the guard; composer no longer disabled while a turn runs. |
| `BridgeWorkspace.tsx` (Share) | Same `ComposerInput`, `SendButton` and guard; text stays plain. |
| `SettingsScreens.tsx` | Settings → General → *Return key in messages*: Sends / Adds a line. |
| `styles.css` | Markdown typography, code and table styles, syntax tokens for light and dark, wider Agent column in conversations, composer states, **Newest** button. |
| `package.json` | `react-markdown`, `remark-gfm`, `lowlight` (highlight.js core), `hast-util-to-jsx-runtime` (already a dependency of react-markdown). |

## 5. Validation

- `npx tsc --noEmit`: clean. `npm run build`: passes.
- `npm run test:frontend-integration`: 73/73, including six new tests that render real output with `react-dom/server`: GFM elements and code (aliases, unknown languages, size limit); hostile input (HTML, `javascript:`, `data:`, `file:`, relative and fragment links, images, event handlers) produces no element with `href`, `src`, `style` or `on*`; prompts verbatim versus replies as Markdown; the full Enter/IME decision table including WebKit's order; the send guard (in-flight, rejected, synchronous and throwing senders); composer wiring. The existing test that pinned replies as plain text was updated to pin Markdown plus Copy of the exact text.
- natural-v1 (23), natural-v2 (12), Layer 4 matrix (49 + 106 + 16), transfer planner (63): all pass. Rust untouched; not run.
- **Driven in the renderer (Chromium):** typing with real key events — Shift+Return added a line, two Returns sent once, the field stayed focused and read-only while sending, then cleared; replayed IME orders (Chromium `isComposing`; WebKit `compositionend` then `keyCode 229`; WebKit without 229 inside the window) — none sent, a later Return did; ⌘Return sent, a repeated Return did not; switching the preference took effect live; scrolled-up reading stayed put with **Newest**, and one's own send followed. Inspected at 420, 800 and 1440 px wide in light and dark: no horizontal page scroll; code and tables scroll inside their blocks.
- **Not verified:** real IME input in the macOS WKWebView and Windows WebView2 (the WebKit order was replayed, not typed); VoiceOver; the packaged Tauri app (the Host clipboard path for Copy was not exercised).
- Bundle: 831.6 KB → 1,108.7 KB minified (226.7 → 314.8 KB gzip), about 102 KB of it highlight.js core and grammars, loaded from local disk.

## 6. Deferred, by impact and cost

1. **Open links in the browser** (high impact, small-medium): a Host command that opens only http(s), after showing the full address, through the existing opener plugin. New capability, so not done here.
2. **Physical IME check** on macOS and Windows with Pinyin and Japanese (high, small): typing, not replay.
3. **Find in conversation** (medium, medium): long sessions have no search; ⌘F in the webview is not wired.
4. **Lazy-load highlighting** (low, small): split `AgentMarkdown` into its own chunk if startup time is measured to matter.
5. **Per-turn timing** (medium, needs data already in task status): show when a turn started and how long it ran, from Host facts.
6. **Screen-reader announcements** of turn state changes (medium, small): an `aria-live` summary of the turn headline.
