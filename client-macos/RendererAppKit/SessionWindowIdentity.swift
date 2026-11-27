//
// SessionWindowIdentity.swift
// RendererAppKit
//
// The client-owned identity line on every Surface window of one session (PX-095; §6.3, §17).
//
// Several sessions can be open at once, and two hosts can serve the same application with the
// same node IDs, the same process IDs and the same names. The server-owned `label` (the window
// title) cannot tell them apart, so the connection manager, never the server, names the host,
// the account and the session incarnation (§17: `session_id` is the incarnation token) on every
// window the session's renderer mounts. The line is a titlebar accessory below the title, apart
// from both the server's title and PX-022-G01's status subtitle, so none of the three overwrites
// another. Each session's windows also get their own `tabbingIdentifier`, so macOS window tabbing
// never merges two sessions' windows into one tab group.
//

import AppKit

/// What a session's windows say about whose they are (PX-095). Set only by the client's
/// connection manager from its own saved endpoint and the session ID its transport attached to.
public struct SessionWindowIdentity: Equatable, Sendable {
    /// The line shown on every Surface window: host, account and incarnation.
    public let text: String
    /// The windows' `tabbingIdentifier`: unique per session, so no tab group spans two sessions.
    public let groupIdentifier: String

    public init(text: String, groupIdentifier: String) {
        self.text = text
        self.groupIdentifier = groupIdentifier
    }

    /// The accessory's user-interface identifier, by which tests and automation find the line.
    public static let accessoryIdentifier = NSUserInterfaceItemIdentifier("org.srui.session-identity")
}

/// The titlebar accessory that draws a `SessionWindowIdentity` below the window title.
@MainActor
final class SessionIdentityAccessory: NSTitlebarAccessoryViewController {
    let field: NSTextField

    init(text: String) {
        field = NSTextField(labelWithString: text)
        super.init(nibName: nil, bundle: nil)
        field.identifier = SessionWindowIdentity.accessoryIdentifier
        field.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        field.textColor = .secondaryLabelColor
        field.lineBreakMode = .byTruncatingMiddle
        field.translatesAutoresizingMaskIntoConstraints = false
        // VoiceOver reaches the line as one static text element in the titlebar (§22.8).
        field.setAccessibilityElement(true)
        field.setAccessibilityRole(.staticText)
        field.setAccessibilityLabel("Session")
        let container = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: 20))
        container.addSubview(field)
        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 12),
            field.trailingAnchor.constraint(lessThanOrEqualTo: container.trailingAnchor, constant: -12),
            field.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])
        view = container
        layoutAttribute = .bottom
        update(text)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    func update(_ text: String) {
        field.stringValue = text
        field.toolTip = text
        field.setAccessibilityValue(text)
    }
}

extension NSWindow {
    /// The identity line this window shows, or `nil` (PX-095).
    public var sessionIdentityText: String? {
        titlebarAccessoryViewControllers
            .compactMap { $0 as? SessionIdentityAccessory }
            .first?.field.stringValue
    }

    @MainActor
    func applySessionWindowIdentity(_ identity: SessionWindowIdentity?) {
        let existing = titlebarAccessoryViewControllers.compactMap { $0 as? SessionIdentityAccessory }
        guard let identity else {
            for accessory in existing {
                accessory.removeFromParent()
            }
            return
        }
        tabbingIdentifier = identity.groupIdentifier
        if let accessory = existing.first {
            accessory.update(identity.text)
        } else {
            addTitlebarAccessoryViewController(SessionIdentityAccessory(text: identity.text))
        }
    }
}
