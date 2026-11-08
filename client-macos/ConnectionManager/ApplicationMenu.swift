//
// ApplicationMenu.swift
// ConnectionManager
//
// Standard macOS application menu for the custom NSApplication lifecycle.
//
// The Edit menu (PX-016-G01; §22.5, §22.6) gives every native text control the standard
// Undo/Redo/Cut/Copy/Paste/Select All key equivalents. Each item targets the first responder
// (`target == nil`), so AppKit routes it to whichever local editor is focused; nothing here is
// remote, and a semantic `shortcut` can never claim these combinations (they are in the macOS
// reserved table, SemanticModel/Shortcut.swift).
//

import AppKit

@MainActor
public enum SRUIApplicationMenu {
    public static func install(
        on application: NSApplication,
        applicationName: String = ProcessInfo.processInfo.processName
    ) {
        application.mainMenu = make(
            applicationName: applicationName,
            target: application
        )
    }

    static func make(
        applicationName: String,
        target: NSApplication
    ) -> NSMenu {
        let mainMenu = NSMenu()
        let applicationMenuItem = NSMenuItem(
            title: applicationName,
            action: nil,
            keyEquivalent: ""
        )
        let applicationMenu = NSMenu(title: applicationName)
        let quitItem = NSMenuItem(
            title: "Quit \(applicationName)",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        quitItem.keyEquivalentModifierMask = [.command]
        quitItem.target = target
        applicationMenu.addItem(quitItem)
        applicationMenuItem.submenu = applicationMenu
        mainMenu.addItem(applicationMenuItem)
        mainMenu.addItem(makeEditMenuItem())
        return mainMenu
    }

    /// The standard Edit menu; every item is sent to the first responder.
    static func makeEditMenuItem() -> NSMenuItem {
        let editMenu = NSMenu(title: "Edit")
        let entries: [(String, Selector, String, NSEvent.ModifierFlags)?] = [
            ("Undo", Selector(("undo:")), "z", [.command]),
            ("Redo", Selector(("redo:")), "z", [.command, .shift]),
            nil,
            ("Cut", #selector(NSText.cut(_:)), "x", [.command]),
            ("Copy", #selector(NSText.copy(_:)), "c", [.command]),
            ("Paste", #selector(NSText.paste(_:)), "v", [.command]),
            ("Select All", #selector(NSText.selectAll(_:)), "a", [.command]),
        ]
        for entry in entries {
            guard let (title, action, key, mask) = entry else {
                editMenu.addItem(.separator())
                continue
            }
            let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
            item.keyEquivalentModifierMask = mask
            item.target = nil
            editMenu.addItem(item)
        }
        let editMenuItem = NSMenuItem(title: "Edit", action: nil, keyEquivalent: "")
        editMenuItem.submenu = editMenu
        return editMenuItem
    }
}
