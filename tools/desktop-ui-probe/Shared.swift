// Shared Accessibility helpers for the desktop UI probes (research only).
//
// Everything here goes through the public macOS Accessibility API
// (AXUIElement). Nothing reads the app's files, database, IPC or credentials.

import AppKit
import ApplicationServices
import CryptoKit
import Foundation

let codexBundleId = "com.openai.codex"
let arguments = CommandLine.arguments

func option(_ name: String) -> String? {
    guard let index = arguments.firstIndex(of: name), index + 1 < arguments.count else { return nil }
    return arguments[index + 1]
}

/// When false, every string a tool prints is redacted to its length and a
/// short hash, except labels that match known interface words.
var revealText = false

// Interface words whose labels are shown even when redacting.
let interfaceWords = [
    "send", "stop", "new", "search", "settings", "attach", "add", "model", "voice", "dictat",
    "submit", "cancel", "approve", "deny", "reject", "allow", "run", "continue", "retry",
    "copy", "back", "forward", "menu", "sidebar", "close", "minimize", "zoom", "full screen",
    "archive", "delete", "rename", "pin", "share", "more", "options", "threads", "chats",
    "codex", "chatgpt", "project", "branch", "local", "worktree", "cloud", "upload", "file",
    "发送", "停止", "新", "搜索", "设置", "附加", "批准", "拒绝", "允许", "运行", "继续", "重试",
    "复制", "返回", "菜单", "侧边栏", "关闭", "归档", "删除", "重命名", "共享", "更多", "选项",
]

/// Labels of controls that answer a native approval. The tools never press them.
let approvalWords = ["approve", "deny", "reject", "allow", "批准", "拒绝", "允许", "始终", "always"]

func digest(_ text: String) -> String {
    SHA256.hash(data: Data(text.utf8)).prefix(4).map { String(format: "%02x", $0) }.joined()
}

func shown(_ text: String?, interface: Bool = false) -> String? {
    guard let text, !text.isEmpty else { return nil }
    if revealText { return String(text.prefix(400)) }
    let lower = text.lowercased()
    if interface, text.count <= 40, interfaceWords.contains(where: { lower.contains($0) }) {
        return text
    }
    return "<\(text.count) chars #\(digest(text))>"
}

func fail(_ message: String, code: Int32) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(code)
}

func attribute(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func string(_ element: AXUIElement, _ name: String) -> String? {
    attribute(element, name) as? String
}

func bool(_ element: AXUIElement, _ name: String) -> Bool? {
    (attribute(element, name) as? NSNumber)?.boolValue
}

func children(_ element: AXUIElement) -> [AXUIElement] {
    (attribute(element, kAXChildrenAttribute) as? [AXUIElement]) ?? []
}

func frame(_ element: AXUIElement) -> [Int]? {
    guard let position = attribute(element, kAXPositionAttribute),
          let size = attribute(element, kAXSizeAttribute) else { return nil }
    var point = CGPoint.zero
    var extent = CGSize.zero
    AXValueGetValue(position as! AXValue, .cgPoint, &point)
    AXValueGetValue(size as! AXValue, .cgSize, &extent)
    return [Int(point.x), Int(point.y), Int(extent.width), Int(extent.height)]
}

func actions(_ element: AXUIElement) -> [String] {
    var names: CFArray?
    guard AXUIElementCopyActionNames(element, &names) == .success else { return [] }
    return (names as? [String]) ?? []
}

/// Route of a web area's URL with identifiers replaced.
func route(_ element: AXUIElement) -> String? {
    guard let url = attribute(element, kAXURLAttribute) as? URL else { return nil }
    let text = url.absoluteString
    if revealText { return text }
    let pattern = try! NSRegularExpression(pattern: "[0-9a-fA-F-]{12,}|[A-Za-z0-9_-]{24,}")
    return pattern.stringByReplacingMatches(
        in: text, range: NSRange(text.startIndex..., in: text), withTemplate: "<id>")
}

struct Node {
    let element: AXUIElement
    let depth: Int
    let path: String
    let role: String
    let subrole: String?
    let domClasses: String
    let domId: String?
    let rawTitle: String?
    let rawLabel: String?
    let rawValue: String?
    let placeholder: String?
    let enabled: Bool?
    let focused: Bool?
    let selected: Bool?
    let busy: Bool?
    let frame: [Int]?
    let childCount: Int
    let actions: [String]
    let url: String?

    var interface: Bool { interfaceRoles.contains(role) }
    var title: String? { shown(rawTitle, interface: interface) }
    var label: String? { shown(rawLabel, interface: interface) }
    var value: String? { shown(rawValue) }
    var valueLength: Int { rawValue?.count ?? 0 }
    /// The control's name: its title, else its description.
    var name: String { rawTitle ?? rawLabel ?? "" }
    func contains(_ other: Node) -> Bool { other.path.hasPrefix(path + ".") }
}

let interfaceRoles: Set<String> = [
    "AXButton", "AXMenuButton", "AXPopUpButton", "AXCheckBox", "AXRadioButton", "AXTab",
    "AXMenuItem", "AXDisclosureTriangle",
]

func walk(_ root: AXUIElement, limit: Int = 20000, maxDepth: Int = 80) -> [Node] {
    var nodes: [Node] = []
    func visit(_ element: AXUIElement, _ depth: Int, _ path: String) {
        guard nodes.count < limit, depth <= maxDepth else { return }
        let role = string(element, kAXRoleAttribute) ?? "?"
        let kids = children(element)
        let classes = (attribute(element, "AXDOMClassList") as? [String])?.joined(separator: " ")
        nodes.append(Node(
            element: element,
            depth: depth,
            path: path,
            role: role,
            subrole: string(element, kAXSubroleAttribute),
            domClasses: String((classes ?? "").prefix(160)),
            domId: string(element, "AXDOMIdentifier"),
            rawTitle: string(element, kAXTitleAttribute),
            rawLabel: string(element, kAXDescriptionAttribute),
            rawValue: attribute(element, kAXValueAttribute) as? String,
            placeholder: shown(string(element, kAXPlaceholderValueAttribute), interface: true),
            enabled: bool(element, kAXEnabledAttribute),
            focused: bool(element, kAXFocusedAttribute),
            selected: bool(element, kAXSelectedAttribute),
            busy: bool(element, "AXElementBusy"),
            frame: frame(element),
            childCount: kids.count,
            actions: actions(element).filter { !$0.hasPrefix("AXScrollToVisible") },
            url: role == "AXWebArea" ? route(element) : nil))
        for (index, child) in kids.enumerated() {
            visit(child, depth + 1, "\(path).\(index)")
        }
    }
    visit(root, 0, "w")
    return nodes
}

func describe(_ node: Node) -> String {
    var parts = [String(repeating: "  ", count: node.depth) + node.role]
    if let subrole = node.subrole { parts.append("sub=\(subrole)") }
    if let domId = node.domId, !domId.isEmpty { parts.append("#\(domId)") }
    if !node.domClasses.isEmpty { parts.append(".\(node.domClasses)") }
    if let title = node.title { parts.append("title=\(title)") }
    if let label = node.label { parts.append("label=\(label)") }
    if let value = node.value { parts.append("value=\(value)") }
    if let placeholder = node.placeholder { parts.append("placeholder=\(placeholder)") }
    if node.enabled == false { parts.append("disabled") }
    if node.focused == true { parts.append("FOCUSED") }
    if node.selected == true { parts.append("selected") }
    if node.busy == true { parts.append("BUSY") }
    if let url = node.url { parts.append("url=\(url)") }
    if !node.actions.isEmpty { parts.append("actions=\(node.actions.joined(separator: ","))") }
    if let frame = node.frame { parts.append("frame=\(frame)") }
    parts.append("kids=\(node.childCount) @\(node.path)")
    return parts.joined(separator: " ")
}

/// The desktop app's semantic regions, found by role and stable class names.
struct CodexView {
    let nodes: [Node]
    let windowTitle: String?

    /// The open thread's title, which the app sets as its web area's title.
    var threadTitle: String? { nodes.first { $0.role == "AXWebArea" }?.rawTitle }
    var threadContainer: Node? { nodes.first { $0.domClasses.contains("thread-scroll-container") } }
    var composer: Node? { nodes.first { $0.domClasses.contains("ProseMirror") } }
    var sidebarItems: [Node] { nodes.filter { $0.role == "AXButton" && $0.domClasses.contains("sidebar-item") } }

    /// The composer's text: its value, or the text under it. An empty
    /// composer shows only a `.placeholder` element, whose text is then also
    /// the composer's value; ProseMirror ends the value with a newline.
    var composerText: String {
        guard let composer else { return "" }
        if nodes.contains(where: { composer.contains($0) && $0.domClasses.split(separator: " ").contains("placeholder") }) {
            return ""
        }
        if let value = composer.rawValue, !value.isEmpty {
            return value.hasSuffix("\n") ? String(value.dropLast()) : value
        }
        return nodes.filter { composer.contains($0) && $0.role == "AXStaticText" }
            .compactMap(\.rawValue).joined()
    }

    /// The composer's form: the nearest ancestor of the composer that also
    /// holds a Send or Stop button.
    var composerForm: String? {
        guard let composer else { return nil }
        var prefix = composer.path
        while let dot = prefix.lastIndex(of: ".") {
            prefix = String(prefix[..<dot])
            if nodes.contains(where: { $0.path.hasPrefix(prefix + ".") && (isSend($0) || isStop($0)) }) {
                return prefix
            }
        }
        return nil
    }

    var composerButtons: [Node] {
        guard let form = composerForm else { return [] }
        return nodes.filter { $0.path.hasPrefix(form + ".") && ["AXButton", "AXPopUpButton"].contains($0.role) }
    }

    func isSend(_ node: Node) -> Bool {
        let name = node.name.lowercased()
        return node.role == "AXButton" && (name.hasPrefix("send") || name.hasPrefix("发送"))
    }

    func isStop(_ node: Node) -> Bool {
        let name = node.name.lowercased()
        return node.role == "AXButton" && (name.hasPrefix("stop") || name == "停止")
    }

    var sendButton: Node? { composerButtons.first(where: isSend) }
    var running: Bool { composerButtons.contains(where: isStop) }

    /// Visible controls whose names look like an approval answer.
    var approvalControls: [Node] {
        nodes.filter { node in
            node.interface && approvalWords.contains { node.name.lowercased().contains($0) }
        }
    }

    /// The reply to the message containing `marker`: the text after the
    /// first heading that follows it, within that heading's message list,
    /// without the headings' own text, up to the next heading with the same
    /// role text as the message's own (the next turn by that sender). Text
    /// outside the list (such as a notice under the thread) and the composer
    /// are excluded. The marker's first occurrence is the sent message: a
    /// reply may quote it.
    func reply(after marker: String) -> String? {
        guard let container = threadContainer,
              let sent = nodes.firstIndex(where: { container.contains($0) && $0.role == "AXStaticText" && ($0.rawValue ?? "").contains(marker) }),
              let heading = nodes[sent...].firstIndex(where: { container.contains($0) && $0.role == "AXHeading" })
        else { return nil }
        let list = String(nodes[heading].path[..<nodes[heading].path.lastIndex(of: ".")!])
        let headings = nodes.filter { $0.role == "AXHeading" && $0.path.hasPrefix(list + ".") }
        func roleText(_ heading: Node) -> String {
            nodes.filter { heading.contains($0) && $0.role == "AXStaticText" }.compactMap(\.rawValue).joined()
        }
        let senderRole = nodes[..<sent].last(where: { $0.role == "AXHeading" && $0.path.hasPrefix(list + ".") }).map(roleText)
        var texts: [String] = []
        for node in nodes[(heading + 1)...] where node.path.hasPrefix(list + ".") {
            if node.role == "AXHeading", let senderRole, roleText(node) == senderRole { break }
            if node.role == "AXStaticText" && !headings.contains(where: { $0.contains(node) }) {
                texts.append(node.rawValue ?? "")
            }
        }
        return texts.joined(separator: "\n")
    }

    /// Thread text in tree order, excluding the composer's form.
    var threadTexts: [Node] {
        guard let container = threadContainer else { return [] }
        let form = composerForm
        return nodes.filter { node in
            container.contains(node) && node.role == "AXStaticText"
                && !(form.map { node.path.hasPrefix($0 + ".") } ?? false)
        }
    }
}

final class CodexApp {
    let app: NSRunningApplication
    let element: AXUIElement
    private let wasManual: Bool
    let manualResult: AXError

    init() {
        guard AXIsProcessTrusted() else { fail("This process is not trusted for Accessibility.", code: 2) }
        guard let app = NSWorkspace.shared.runningApplications.first(where: { $0.bundleIdentifier == codexBundleId }) else {
            fail("The ChatGPT / Codex desktop app is not running.", code: 3)
        }
        self.app = app
        element = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(element, 3)
        // Electron's documented switch for assistive technology. This build
        // answers "unsupported" and exposes its tree regardless.
        wasManual = bool(element, "AXManualAccessibility") ?? false
        manualResult = AXUIElementSetAttributeValue(element, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        if manualResult == .success && !wasManual { Thread.sleep(forTimeInterval: 1.5) }
    }

    func restore() {
        if manualResult == .success && !wasManual {
            AXUIElementSetAttributeValue(element, "AXManualAccessibility" as CFString, kCFBooleanFalse)
        }
    }

    var windows: [AXUIElement] { (attribute(element, kAXWindowsAttribute) as? [AXUIElement]) ?? [] }
    var frontmost: Bool { NSWorkspace.shared.frontmostApplication?.bundleIdentifier == codexBundleId }

    /// The main window's view. The probes use the first standard window.
    func view() -> CodexView? {
        guard let window = windows.first(where: { string($0, kAXSubroleAttribute) == "AXStandardWindow" }) ?? windows.first
        else { return nil }
        return CodexView(nodes: walk(window), windowTitle: string(window, kAXTitleAttribute))
    }
}
