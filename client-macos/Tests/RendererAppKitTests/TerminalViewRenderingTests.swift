import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

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
}
