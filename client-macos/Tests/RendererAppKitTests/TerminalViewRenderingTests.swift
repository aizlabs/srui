import AppKit
import Foundation
import SemanticModel
import Terminal
import Testing
@testable import RendererAppKit

@MainActor
private func terminalViewBitmapSignature(_ view: NSView) -> Data? {
    view.layoutSubtreeIfNeeded()
    let rect = view.bounds.integral
    guard rect.width > 0, rect.height > 0,
          let representation = view.bitmapImageRepForCachingDisplay(in: rect) else {
        return nil
    }
    view.cacheDisplay(in: rect, to: representation)
    guard let bytes = representation.bitmapData else { return nil }
    return Data(
        bytes: bytes,
        count: representation.bytesPerRow * representation.pixelsHigh
    )
}

private func terminalSnapshotFullPlainText(_ snapshot: TerminalSnapshot) -> String {
    (snapshot.scrollback + snapshot.cells)
        .map { row in
            var characters = row.map(\.character)
            while let last = characters.last, last.isWhitespace {
                characters.removeLast()
            }
            return String(characters)
        }
        .joined(separator: "\n")
        .trimmingCharacters(in: CharacterSet.newlines.union(.whitespaces))
}

@MainActor
struct TerminalViewRenderingTests {
    @Test
    func clipsDirtyDrawingToItsOwnBounds() {
        let view = TerminalView(nodeID: 1)
        view.frame = NSRect(x: 40, y: 50, width: 120, height: 80)

        let oversizedDirtyRect = NSRect(x: -400, y: -300, width: 1_000, height: 900)

        #expect(view.clippedDirtyRect(oversizedDirtyRect) == view.bounds)
        #expect(view.layer?.masksToBounds == true)
    }

    @Test
    func publishesLargerGridWhenBoundsGrow() async throws {
        let view = TerminalView(nodeID: 1)
        var reportedSizes: [(columns: UInt32, rows: UInt32)] = []
        view.onResize = { columns, rows, _, _ in
            reportedSizes.append((columns, rows))
        }

        view.setFrameSize(NSSize(width: 656, height: 480))
        view.layout()
        try await Task.sleep(nanoseconds: 100_000_000)
        let initial = try #require(reportedSizes.last)

        view.setFrameSize(NSSize(width: 800, height: 600))
        view.layout()
        try await Task.sleep(nanoseconds: 100_000_000)
        let enlarged = try #require(reportedSizes.last)

        #expect(initial.columns >= TerminalView.conventionalColumns)
        #expect(initial.rows >= TerminalView.conventionalRows)
        #expect(enlarged.columns > initial.columns)
        #expect(enlarged.rows > initial.rows)
    }

    @Test
    func standaloneSessionPublishesExactANSIContentIntoDrawableView() async throws {
        let streamID = NodeId(14)
        let payload = Data(
            "\u{1b}[32mfirst line\u{1b}[0m\r\n\u{1b}[32msecond line\u{1b}[0m".utf8
        )
        let session = TerminalSession()
        await session.resize(streamID: streamID, columns: 80, rows: 24)
        let snapshots = await session.snapshots(for: streamID)
        var iterator = snapshots.makeAsyncIterator()
        let initialOptional = await iterator.next()
        let initial = try #require(initialOptional)

        let view = TerminalView(nodeID: streamID)
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 384),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentView = view
        defer {
            window.contentView = nil
            window.close()
        }

        view.apply(initial)
        let blank = try #require(terminalViewBitmapSignature(view))
        let applied = try await session.applyData(
            streamID: streamID,
            byteOffset: 0,
            data: payload
        )
        let publishedOptional = await iterator.next()
        let published = try #require(publishedOptional)
        view.apply(published)
        let rendered = try #require(terminalViewBitmapSignature(view))

        #expect(published.nextOffset == UInt64(payload.count))
        #expect(published.nextOffset == applied.nextOffset)
        #expect(terminalSnapshotFullPlainText(published) == "first line\nsecond line")
        #expect(view.accessibilityValue() as? String == published.plainText())
        #expect(rendered != blank)
    }
}
