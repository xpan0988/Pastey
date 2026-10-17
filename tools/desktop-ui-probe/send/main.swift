// Guarded one-shot send into one existing ChatGPT / Codex desktop thread
// (research PoC). Use only on a test conversation the user authorized.
//
// Through the public Accessibility API only, it:
//   1. opens the thread whose sidebar title is exactly --thread-title, and
//      confirms the open thread's title matches;
//   2. refuses unless no turn is running, the composer is empty, no
//      approval control is shown, and the nonce is not already in the thread;
//   3. puts --message into the composer and reads it back exactly;
//   4. presses Send once, and never again: if the send cannot be confirmed
//      it reports "unknown" and stops;
//   5. waits for the turn to end and prints the reply.
// It never presses an approval control. A lock file allows one sender at a
// time. --dry-run stops before step 3 and changes nothing (except opening the
// thread, which it reports instead of doing).
//
//   send --thread-title T --message M --nonce N [--dry-run] [--timeout S] [--front-wait S] [--out DIR]

import AppKit
import Foundation

guard let threadTitle = option("--thread-title"), let message = option("--message"),
      let nonce = option("--nonce"), nonce.count >= 6, message.contains(nonce)
else { fail("usage: send --thread-title T --message M (containing --nonce N, 6+ chars) [--dry-run]", code: 64) }
let dryRun = arguments.contains("--dry-run")
let timeout = Double(option("--timeout") ?? "300") ?? 300
let frontWait = Double(option("--front-wait") ?? "0") ?? 0
let outDir = URL(fileURLWithPath: option("--out") ?? FileManager.default.currentDirectoryPath)
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

// One sender at a time, across processes.
let lockPath = outDir.appendingPathComponent("send.lock").path
let lock = open(lockPath, O_CREAT | O_EXCL | O_WRONLY, 0o600)
guard lock >= 0 else { fail("Another sender holds \(lockPath); not sending.", code: 5) }
close(lock)
func finish(_ code: Int32, _ line: String) -> Never {
    print(line)
    unlink(lockPath)
    exit(code)
}

let codex = CodexApp()
var log: [String] = []
func note(_ line: String) {
    let stamped = "[\(log.count)] \(line)"
    log.append(stamped)
    print(stamped)
    fflush(stdout)
    try? log.joined(separator: "\n").write(
        to: outDir.appendingPathComponent("send-log.txt"), atomically: true, encoding: .utf8)
}

func current() -> CodexView {
    guard let view = codex.view() else { finish(4, "No window.") }
    return view
}

func wait(_ seconds: Double, until condition: (CodexView) -> Bool) -> CodexView? {
    let end = Date().addingTimeInterval(seconds)
    while Date() < end {
        let view = current()
        if condition(view) { return view }
        Thread.sleep(forTimeInterval: 0.25)
    }
    return nil
}

// 1. The authorized thread, by exact title.
var view = current()
note("front=\(codex.frontmost) open_thread_is_target=\(view.threadTitle == threadTitle)")
if view.threadTitle != threadTitle {
    let matches = view.sidebarItems.filter { $0.name == threadTitle }
    guard matches.count == 1 else { finish(6, "Expected one sidebar thread titled exactly as given; found \(matches.count).") }
    if dryRun { finish(0, "dry-run: would open the target thread from the sidebar; it is not open now.") }
    let pressed = AXUIElementPerformAction(matches[0].element, kAXPressAction as CFString)
    note("opened target thread from the sidebar: \(pressed == .success ? "ok" : "error \(pressed.rawValue)")")
    guard let opened = wait(5, until: { $0.threadTitle == threadTitle }) else {
        finish(7, "The target thread did not open; not sending.")
    }
    view = opened
}

// From here on, text of this thread may be shown: the user authorized it.
revealText = true

// 2. Preconditions.
let texts = view.threadTexts.compactMap(\.rawValue)
note("thread_texts=\(texts.count) running=\(view.running) composer_chars=\(view.composerText.count) send=\(view.sendButton.map { "\($0.name) enabled=\($0.enabled ?? true)" } ?? "none") approvals=\(view.approvalControls.map(\.name))")
note("composer_buttons=\(view.composerButtons.map { "\($0.name)\($0.enabled == false ? " (disabled)" : "")" })")
note("composer_text_when_empty=\(String(reflecting: view.composerText)) placeholder=\(view.composer?.placeholder ?? "-")")
guard !view.running else { finish(8, "A turn is running in this thread; not sending.") }
guard view.approvalControls.isEmpty else { finish(9, "An approval control is shown; not sending and not answering it.") }
guard !texts.contains(where: { $0.contains(nonce) }) else { finish(10, "The nonce is already in this thread; it was sent before. Not sending again.") }
guard let composer = view.composer else { finish(11, "No composer found.") }
// An earlier unsent attempt may have left exactly this message there.
let alreadyTyped = view.composerText == message
guard view.composerText.isEmpty || alreadyTyped else { finish(12, "The composer holds other text (a draft?); not touching it.") }
guard view.sendButton != nil else { finish(13, "No Send button found next to the composer.") }
if dryRun { finish(0, "dry-run: all preconditions hold\(alreadyTyped ? " (message already in the composer)" : ""); nothing typed or sent.") }

// 3. Put the message into the composer, then read it back.
AXUIElementSetAttributeValue(composer.element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
if alreadyTyped { note("composer already holds the exact message; not typing it again") }
var method = "AXSelectedText"
var result = AXError.success
var typed = alreadyTyped ? view : nil
if !alreadyTyped {
    result = AXUIElementSetAttributeValue(composer.element, kAXSelectedTextAttribute as CFString, message as CFString)
    typed = wait(2, until: { $0.composerText == message })
}
if typed == nil {
    note("AXSelectedText gave \(result.rawValue); composer reads \(String(reflecting: current().composerText))")
    guard current().composerText.isEmpty else { finish(14, "The composer now holds unexpected text; not sending. Clear it by hand.") }
    method = "AXValue"
    result = AXUIElementSetAttributeValue(composer.element, kAXValueAttribute as CFString, message as CFString)
    typed = wait(2, until: { $0.composerText == message })
}
guard let ready = typed else {
    finish(15, "The composer does not read back the exact message (\(method) gave \(result.rawValue)); not sending. It reads \(String(reflecting: current().composerText)).")
}
note("composer holds the exact message via \(method)")
guard let send = ready.sendButton, send.enabled != false else {
    finish(16, "Send is not enabled after typing; the app did not take the input. Not sending.")
}

// 4. Send exactly once. The app's buttons may list no AXPress, and calling
// it anyway reports success without effect, so press only when listed.
// Otherwise submit with one Return key in the composer: bring the app to
// the front, check again that the composer has focus and holds exactly the
// message, then post the key to the app alone.
let sentAt: Date
if send.actions.contains(kAXPressAction as String) {
    sentAt = Date()
    let press = AXUIElementPerformAction(send.element, kAXPressAction as CFString)
    note("pressed Send once: \(press == .success ? "ok" : "error \(press.rawValue)")")
} else {
    note("Send lists no AXPress (actions: \(send.actions)); submitting with one Return key")
    // macOS does not let a background process take the front, so the user
    // brings the app forward within --front-wait seconds.
    AXUIElementSetAttributeValue(codex.element, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    if !codex.frontmost { note("waiting up to \(Int(frontWait)) s for the app to be brought to the front") }
    guard wait(frontWait + 3, until: { _ in codex.frontmost }) != nil else {
        finish(19, "The app was not in front; no key was sent.")
    }
    AXUIElementSetAttributeValue(composer.element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    guard let final = wait(2, until: { $0.composer?.focused == true && $0.composerText == message }), codex.frontmost,
          !final.running, final.approvalControls.isEmpty
    else { finish(20, "Just before the key, the composer was not focused with exactly the message (or the app lost the front); no key was sent.") }
    let source = CGEventSource(stateID: .privateState)
    let down = CGEvent(keyboardEventSource: source, virtualKey: 36, keyDown: true)!
    let up = CGEvent(keyboardEventSource: source, virtualKey: 36, keyDown: false)!
    down.flags = []
    up.flags = []
    sentAt = Date()
    down.postToPid(codex.app.processIdentifier)
    up.postToPid(codex.app.processIdentifier)
    note("posted one Return key to the app (pid \(codex.app.processIdentifier))")
}
guard let confirmed = wait(15, until: { view in
    view.composerText.isEmpty && view.threadTexts.contains { ($0.rawValue ?? "").contains(nonce) }
}) else {
    finish(17, "Send could not be confirmed (no nonce in the thread, or composer not cleared). Outcome unknown: do NOT send again; check the app.")
}
note("send confirmed after \(Int(Date().timeIntervalSince(sentAt) * 1000)) ms: nonce in thread, composer cleared, running=\(confirmed.running)")

// 5. Wait for the turn to end; never answer an approval.
var sawRunning = confirmed.running
var reportedApproval = false
var last = confirmed
let end = sentAt.addingTimeInterval(timeout)
while Date() < end {
    last = current()
    if last.running { sawRunning = true }
    if !last.approvalControls.isEmpty && !reportedApproval {
        reportedApproval = true
        note("approval shown and left unanswered: \(last.approvalControls.map(\.name))")
    }
    if sawRunning && !last.running { break }
    Thread.sleep(forTimeInterval: 1)
}
let elapsed = Int(Date().timeIntervalSince(sentAt))
let reply = last.reply(after: nonce) ?? ""
note("turn \(last.running ? "still running at timeout" : "ended") after \(elapsed) s; saw_running=\(sawRunning)")
note("reply (\(reply.count) chars):\n\(reply.prefix(2000))")
finish(last.running ? 18 : 0, "done")
