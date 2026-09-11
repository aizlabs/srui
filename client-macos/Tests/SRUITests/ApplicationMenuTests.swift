//
// ApplicationMenuTests.swift
// SRUITests
//

import AppKit
@testable import ConnectionManager
import Testing

@Suite("Application Menu Tests")
struct ApplicationMenuTests {
    @Test("Application menu exposes the standard Command-Q quit action")
    @MainActor
    func commandQQuitAction() throws {
        let application = NSApplication.shared
        let menu = SRUIApplicationMenu.make(
            applicationName: "SRUI Test",
            target: application
        )

        let applicationMenu = try #require(menu.items.first?.submenu)
        let quitItem = try #require(applicationMenu.items.first)

        #expect(quitItem.title == "Quit SRUI Test")
        #expect(quitItem.action == #selector(NSApplication.terminate(_:)))
        #expect(quitItem.keyEquivalent == "q")
        #expect(quitItem.keyEquivalentModifierMask == [.command])
        #expect(quitItem.target === application)
    }
}
