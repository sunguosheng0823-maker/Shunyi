// Disposable macOS input receiver. Launch only with approval to take foreground focus.
import AppKit

let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let eventsURL = root.appendingPathComponent("control-events.jsonl")
FileManager.default.createFile(atPath: eventsURL.path, contents: nil, attributes: [.posixPermissions: 0o600])
let output = try FileHandle(forWritingTo: eventsURL)
class TestView: NSView {
    var eventCount = 0
    override var acceptsFirstResponder: Bool { true }
    func record(_ event: String, code: Int = 0) {
        let line = try! JSONSerialization.data(withJSONObject: ["event": event, "code": code, "time": Date().timeIntervalSince1970])
        output.write(line); output.write(Data([10])); output.synchronizeFile()
        eventCount += 1; needsDisplay = true
    }
    override func keyDown(with event: NSEvent) { record("key_down", code: Int(event.keyCode)) }
    override func keyUp(with event: NSEvent) { record("key_up", code: Int(event.keyCode)) }
    override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self); record("mouse_down") }
    override func mouseUp(with event: NSEvent) { record("mouse_up") }
    override func draw(_ dirtyRect: NSRect) {
        NSColor(calibratedRed: 0.08, green: 0.12, blue: 0.19, alpha: 1).setFill(); bounds.fill()
        let title = "瞬移 · 独立键鼠验收窗口"
        title.draw(at: NSPoint(x: 35, y: 185), withAttributes: [.font: NSFont.systemFont(ofSize: 26, weight: .semibold), .foregroundColor: NSColor.white])
        "这里只接收本次测试输入，不修改任何文档。".draw(at: NSPoint(x: 35, y: 135), withAttributes: [.font: NSFont.systemFont(ofSize: 17), .foregroundColor: NSColor.lightGray])
        "已接收事件：\(eventCount)".draw(at: NSPoint(x: 35, y: 80), withAttributes: [.font: NSFont.monospacedDigitSystemFont(ofSize: 22, weight: .medium), .foregroundColor: NSColor.systemGreen])
    }
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 700, height: 280), styleMask: [.titled, .closable], backing: .buffered, defer: false)
window.title = "Shunyi Control Acceptance"
window.center()
let view = TestView(frame: window.contentView!.bounds)
window.contentView = view
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(view)
app.activate(ignoringOtherApps: true)
let screen = NSScreen.main!.frame
DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
    let point: [String: Double] = ["x": window.frame.midX, "y": screen.maxY - window.frame.midY, "pid": Double(ProcessInfo.processInfo.processIdentifier), "front_pid": Double(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1)]
    try! JSONSerialization.data(withJSONObject: point).write(to: root.appendingPathComponent("control-target.json"), options: .atomic)
}
app.run()
