//
// ConnectSheetNativeTests.swift
// SRUITests
//
// PX-023: the Connect form as native AppKit controls (D2 T38). The form's text fields are typed
// into through their field editors, and Connect is reached through its Return key equivalent, the
// way a user submits a sheet. A malformed endpoint never submits; a valid one submits exactly what
// was typed. No run loop is serviced (scripts/check-test-main-runloop-nesting.sh).
//

import AppKit
@testable import ConnectionManager
import RendererAppKit
import SwiftUI
import Testing

@MainActor
private final class SubmittedDrafts {
    var drafts: [ConnectDraft] = []
}

@Suite("Connect form native controls (PX-023)", .serialized)
@MainActor
struct ConnectSheetNativeTests {
    private static func textFields(in view: NSView) -> [NSTextField] {
        var found: [NSTextField] = []
        func visit(_ view: NSView) {
            if let field = view as? NSTextField, field.isEditable {
                found.append(field)
                return
            }
            view.subviews.forEach(visit)
        }
        visit(view)
        return found
    }

    /// Replaces the field's text as typing does: through its field editor, so the field posts
    /// the text-change notifications SwiftUI's binding listens to.
    private static func type(_ text: String, into field: NSTextField, in window: NSWindow) throws {
        #expect(window.makeFirstResponder(field))
        let editor = try #require(field.currentEditor() as? NSTextView, "the field has an editor")
        editor.selectAll(nil)
        editor.insertText(text, replacementRange: editor.selectedRange())
    }

    private static func pressReturn(in window: NSWindow) throws -> Bool {
        let event = try #require(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
            windowNumber: window.windowNumber, context: nil, characters: "\r",
            charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
        return window.performKeyEquivalent(with: event)
    }

    private static func settle(_ host: NSView) async throws {
        try await Task.sleep(for: .milliseconds(150))
        host.layoutSubtreeIfNeeded()
        host.window?.displayIfNeeded()
    }

    private static func capture(_ window: NSWindow, name: String) {
        let prefix = ProcessInfo.processInfo.environment["PX023_EVIDENCE_PREFIX"] ?? "px023"
        guard let directory = ProcessInfo.processInfo.environment["PX023_EVIDENCE_DIR"],
              let frameView = window.contentView?.superview,
              let bitmap = frameView.bitmapImageRepForCachingDisplay(in: frameView.bounds) else { return }
        frameView.cacheDisplay(in: frameView.bounds, to: bitmap)
        try? bitmap.representation(using: .png, properties: [:])?
            .write(to: URL(fileURLWithPath: directory).appendingPathComponent("\(prefix)-\(name).png"))
    }

    @Test("Connect submits only a valid endpoint, exactly as typed")
    func formSubmitsOnlyValidEndpoints() async throws {
        _ = NSApplication.shared
        let submitted = SubmittedDrafts()
        let host = NSHostingView(rootView: ConnectSheet(initialDraft: ConnectDraft()) { submitted.drafts.append($0) })
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 460, height: 360),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.title = "PX-023 Connect form"
        window.contentView = host
        // Ordered in under the host's own presentation policy: a `swift test` host is
        // `.prohibited`, so the window is concealed and never painted where a user can see it
        // (SurfacePresentationTests asserts nothing this process shows is drawn).
        SurfacePresentation.forHostApplication().present(window)
        defer { window.orderOut(nil); window.close() }
        try await Self.settle(host)

        // Host, Remote User, Port, Name, in form order, identified by their prompts.
        let fields = Self.textFields(in: host)
        #expect(fields.map { $0.placeholderString ?? "" } == ["host, user@host:port or [::1]", "", "optional", "optional"])
        try #require(fields.count == 4)
        let (hostField, userField, portField, nameField) = (fields[0], fields[1], fields[2], fields[3])

        // An empty form cannot submit.
        _ = try Self.pressReturn(in: window)
        try await Self.settle(host)
        #expect(submitted.drafts.isEmpty)

        // Shell metacharacters, an option, a bad port: each refused, nothing submitted.
        for (hostText, userText, portText) in [
            ("host.example;id", "alice", ""),
            ("-oProxyCommand=id", "alice", ""),
            ("$(id)", "alice", ""),
            ("host.example", "alice", "65536"),
            ("host.example", "alice", "0"),
            ("::1", "alice", ""),
            // PX-023 round 2: a NUL inside brackets once passed and then aborted the client.
            ("[::\u{0};id -oProxyCommand=x]", "alice", ""),
            ("host.example", "al\u{0}ice", ""),
            ("host.example", "alice", "22\u{0}"),
        ] {
            try Self.type(hostText, into: hostField, in: window)
            try Self.type(userText, into: userField, in: window)
            try Self.type(portText, into: portField, in: window)
            try await Self.settle(host)
            _ = try Self.pressReturn(in: window)
            try await Self.settle(host)
            #expect(submitted.drafts.isEmpty, "\(hostText) \(portText) must not submit")
        }
        try Self.type("host.example;id", into: hostField, in: window)
        try Self.type("", into: portField, in: window)
        try await Self.settle(host)
        Self.capture(window, name: "connect-form-rejected")

        // A valid endpoint, user@host:port in the Host field, submits exactly that draft.
        try Self.type("alice@127.0.0.1:2222", into: hostField, in: window)
        try Self.type("", into: userField, in: window)
        try Self.type("Lab", into: nameField, in: window)
        try await Self.settle(host)
        Self.capture(window, name: "connect-form-valid")
        #expect(try Self.pressReturn(in: window))
        try await Self.settle(host)
        let draft = try #require(submitted.drafts.first)
        #expect(submitted.drafts.count == 1)
        #expect(draft.host == "alice@127.0.0.1:2222")
        #expect(draft.label == "Lab")
        let connection = try #require(draft.savedConnection)
        #expect(connection.host == "127.0.0.1")
        #expect(connection.port == 2222)
        #expect(connection.user == "alice")
        #expect(connection.label == "Lab")
    }
}
