//
// ApplicationMenu.swift
// ConnectionManager
//
// Standard macOS application menu for the custom NSApplication lifecycle.
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
        return mainMenu
    }
}
