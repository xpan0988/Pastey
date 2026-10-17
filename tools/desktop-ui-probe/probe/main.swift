// Read-only Accessibility probe of the ChatGPT / Codex desktop app (research).
//
// It reads the main window's accessibility tree and writes what it finds:
// the conversation list, the open thread, the composer, its buttons, run
// state and message text. It never clicks, types, scrolls or focuses.
// Text is redacted to its length and a hash unless --reveal is given, which
// is for a test conversation the user chose.
//
//   probe summary [--out DIR] [--reveal]
//   probe watch --seconds N [--out DIR]   (one line per second, no text)

import AppKit
import Foundation

let mode = arguments.count > 1 && !arguments[1].hasPrefix("--") ? arguments[1] : "summary"
revealText = arguments.contains("--reveal")
let outDir = URL(fileURLWithPath: option("--out") ?? FileManager.default.currentDirectoryPath)
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

let codex = CodexApp()
defer { codex.restore() }

func viewSummary(_ view: CodexView) -> [String: Any] {
    var roles: [String: Int] = [:]
    for node in view.nodes { roles[node.role, default: 0] += 1 }
    let texts = view.threadTexts
    return [
        "nodes": view.nodes.count,
        "roles": roles,
        "window_title": shown(view.windowTitle) ?? "-",
        "thread_title": shown(view.threadTitle) ?? "-",
        "web_areas": view.nodes.filter { $0.role == "AXWebArea" }.map { $0.url ?? "-" },
        "sidebar_items": view.sidebarItems.count,
        "sidebar_selected": view.sidebarItems.filter { $0.selected == true }.count,
        "thread_container": view.threadContainer?.path ?? "-",
        "thread_headings": view.nodes.filter { node in
            node.role == "AXHeading" && (view.threadContainer.map { $0.contains(node) } ?? false)
        }.map { $0.title ?? "-" },
        "thread_text": ["count": texts.count, "chars": texts.reduce(0) { $0 + $1.valueLength }],
        "composer": [
            "present": view.composer != nil,
            "focused": view.composer?.focused ?? false,
            "chars": view.composerText.count,
            "text": shown(view.composerText) ?? "",
        ],
        "composer_buttons": view.composerButtons.map {
            ["name": shown($0.name, interface: true) ?? "-", "enabled": $0.enabled ?? true]
        },
        "send_button": view.sendButton.map { ["enabled": $0.enabled ?? true] } ?? [:],
        "running": view.running,
        "approval_controls": view.approvalControls.map { shown($0.name, interface: true) ?? "-" },
        "busy_nodes": view.nodes.filter { $0.busy == true }.count,
        "progress_indicators": view.nodes.filter {
            $0.role == "AXProgressIndicator" || $0.role == "AXBusyIndicator"
        }.count,
    ]
}

switch mode {
case "summary":
    let started = Date()
    guard let view = codex.view() else { fail("No window.", code: 4) }
    var report = viewSummary(view)
    report["app_active"] = codex.app.isActive
    report["manual_accessibility_set"] = codex.manualResult == .success ? "ok" : "unsupported (\(codex.manualResult.rawValue))"
    report["windows"] = codex.windows.count
    report["revealed"] = revealText
    report["walk_ms"] = Int(Date().timeIntervalSince(started) * 1000)
    let json = try! JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    try! json.write(to: outDir.appendingPathComponent("summary.json"))
    try! view.nodes.map(describe).joined(separator: "\n").write(
        to: outDir.appendingPathComponent("tree.txt"), atomically: true, encoding: .utf8)
    print(String(data: json, encoding: .utf8)!)
case "watch":
    let seconds = Int(option("--seconds") ?? "30") ?? 30
    var lines: [String] = []
    for second in 0..<seconds {
        let started = Date()
        guard let view = codex.view() else {
            lines.append("t=\(second)s no window")
            print(lines.last!)
            Thread.sleep(forTimeInterval: 1)
            continue
        }
        let texts = view.threadTexts
        let line = [
            "t=\(second)s",
            "front=\(codex.frontmost)",
            "windows=\(codex.windows.count)",
            "nodes=\(view.nodes.count)",
            "thread=\(digest(view.threadTitle ?? ""))",
            "texts=\(texts.count)",
            "chars=\(texts.reduce(0) { $0 + $1.valueLength })",
            "text_hash=\(digest(texts.compactMap(\.rawValue).joined(separator: "\u{1}")))",
            "running=\(view.running)",
            "send_enabled=\(view.sendButton?.enabled.map(String.init) ?? "-")",
            "composer_chars=\(view.composerText.count)",
            "approvals=\(view.approvalControls.count)",
            "walk_ms=\(Int(Date().timeIntervalSince(started) * 1000))",
        ].joined(separator: " ")
        print(line)
        fflush(stdout)
        lines.append(line)
        Thread.sleep(forTimeInterval: max(0, 1 - Date().timeIntervalSince(started)))
    }
    try! lines.joined(separator: "\n").write(
        to: outDir.appendingPathComponent("watch.txt"), atomically: true, encoding: .utf8)
default:
    fail("usage: probe summary|watch [--seconds N] [--out DIR] [--reveal]", code: 64)
}
