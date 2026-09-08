//
// CollectionRangeSocketIntegrationTests.swift
// SRUITests
//
// Live Unix-socket coverage for sparse collection range hydration (§8, §12.1, §22.7).
// Requires the counter debug binary built with `--large-collection-fixture`.
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit
import Collections

@Suite("Collection range socket integration")
struct CollectionRangeSocketIntegrationTests {

    @Test("Large collection fixture hydrates the first page and fills a delayed cache miss")
    @MainActor
    func largeCollectionFixtureHydratesAndFillsCacheMiss() async throws {
        let runtimeDirectory = URL(
            fileURLWithPath: "/tmp/srui-collection-range-\(UUID().uuidString)"
        )
        try FileManager.default.createDirectory(
            at: runtimeDirectory,
            withIntermediateDirectories: false
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: runtimeDirectory.path
        )
        defer { try? FileManager.default.removeItem(at: runtimeDirectory) }
        let socketPath = runtimeDirectory.appendingPathComponent("counter.sock").path
        let repoRoot = Self.repositoryRoot()
        let counterBinary = repoRoot.appendingPathComponent("examples/counter/target/debug/counter")

        try #require(
            FileManager.default.fileExists(atPath: counterBinary.path),
            "counter debug binary missing; run scripts/test_task28_collection_ranges.sh"
        )
        try #require(
            Self.counterSupportsLargeCollectionFixture(counterBinary),
            "counter binary lacks --large-collection-fixture; rebuild examples/counter"
        )

        let server = Process()
        server.executableURL = counterBinary
        server.arguments = ["--socket", socketPath, "--large-collection-fixture"]
        var environment = ProcessInfo.processInfo.environment
        environment["SRUI_COLLECTION_PROVIDER_DELAY_MS"] = "200"
        server.environment = environment
        server.standardOutput = FileHandle.nullDevice
        server.standardError = FileHandle.nullDevice

        try server.run()
        defer {
            if server.isRunning {
                server.terminate()
            }
            server.waitUntilExit()
            try? FileManager.default.removeItem(atPath: socketPath)
        }

        try await Self.waitForSocket(at: socketPath, timeoutSeconds: 10)

        let transport = UnixSocketTransport(socketPath: socketPath)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try await Self.waitForRevision(applier, atLeast: Revision(3), timeoutSeconds: 8)

        let tableID = NodeId(10)
        // The applier commits ahead of the renderer: the mount is a separate main-actor
        // hop, so poll for the handle instead of assuming it landed with the revision.
        try await AsyncTestSupport.eventually(
            timeout: .seconds(8),
            description: "table node mounted"
        ) {
            renderer.registry.handle(for: tableID) != nil
        }
        let handle = try #require(renderer.registry.handle(for: tableID))
        let scroll = try #require(handle.view as? NSScrollView)
        let tableView = try #require(scroll.documentView as? NSTableView)
        let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)
        let tableBefore = tableView

        #expect(type(of: tableView) == NSTableView.self)
        #expect(exactViewCount(NSTableView.self, in: scroll) == 1)
        #expect(exactViewCount(NSOutlineView.self, in: scroll) == 0)
        #expect(adapter.numberOfRows(in: tableView) == 500_000)
        #expect(scroll.hasVerticalScroller)
        #expect(adapter.rowContent(at: 0)?.cells.first == "0")
        #expect(adapter.rowContent(at: 0)?.cells.last == "Row 0")
        #expect(adapter.rowContent(at: 63)?.itemID == ItemId(64))

        let distant = try #require(
            adapter.tableView(tableView, viewFor: tableView.tableColumns[0], row: 10_000) as? NSTextField
        )
        #expect(distant.stringValue == CollectionCells.loadingPlaceholder)
        #expect(adapter.tableView(tableView, shouldSelectRow: 10_000) == false)

        let revisionBeforeRequest = applier.lastAppliedRevision
        adapter.noteVisibleRange(start: 10_000, count: 16)
        #expect(adapter.rowContent(at: 10_000)?.itemID == nil)

        try await Self.waitForRevision(
            applier,
            atLeast: Revision(revisionBeforeRequest.value + 1),
            timeoutSeconds: 8
        )
        try await AsyncTestSupport.eventually(description: "delayed range painted") {
            adapter.rowContent(at: 10_000)?.cells.last == "Row 10000"
        }
        #expect(adapter.rowContent(at: 10_000)?.itemID == ItemId(10_001))
        let tableAfter = try #require((renderer.registry.view(for: tableID) as? NSScrollView)?.documentView as? NSTableView)
        #expect(tableAfter === tableBefore)
        #expect(adapter.numberOfRows(in: tableAfter) == 500_000)

        await controller.stop()
    }

    private static func repositoryRoot() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private static func counterSupportsLargeCollectionFixture(_ binary: URL) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/strings")
        process.arguments = [binary.path]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            let text = String(data: data, encoding: .utf8) ?? ""
            return text.contains("large-collection-fixture")
        } catch {
            return false
        }
    }

    private static func waitForSocket(at path: String, timeoutSeconds: TimeInterval) async throws {
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while Date() < deadline {
            if FileManager.default.fileExists(atPath: path) {
                return
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        throw CollectionRangeSocketError.socketTimeout(path)
    }

    private static func waitForRevision(
        _ applier: TransactionApplier,
        atLeast expected: Revision,
        timeoutSeconds: TimeInterval
    ) async throws {
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while Date() < deadline {
            if applier.lastAppliedRevision.value >= expected.value {
                return
            }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw CollectionRangeSocketError.revisionTimeout(
            expected: expected,
            actual: applier.lastAppliedRevision
        )
    }
}

private func exactViewCount<T: NSView>(_ type: T.Type, in root: NSView) -> Int {
    var count = 0
    if ObjectIdentifier(Swift.type(of: root)) == ObjectIdentifier(type) {
        count += 1
    }
    for child in root.subviews {
        count += exactViewCount(type, in: child)
    }
    return count
}

private enum CollectionRangeSocketError: Error, CustomStringConvertible {
    case socketTimeout(String)
    case revisionTimeout(expected: Revision, actual: Revision)

    var description: String {
        switch self {
        case .socketTimeout(let path):
            return "Timed out waiting for Unix socket at \(path)"
        case .revisionTimeout(let expected, let actual):
            return "Timed out waiting for revision \(expected), still at \(actual)"
        }
    }
}
