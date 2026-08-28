import Foundation
import OSLog

/// Opt-in renderer diagnostics for development.
///
/// Set `SRUI_RENDER_LOGS=1` (also accepts `true` or `yes`) in the process environment
/// to emit mount, mutation, layout, and demo verification details to standard error.
@MainActor
enum RendererDiagnostics {
    private static let logger = Logger(
        subsystem: "org.srui.RendererAppKit",
        category: "render"
    )

    static let isEnabled: Bool = {
        guard let rawValue = ProcessInfo.processInfo.environment["SRUI_RENDER_LOGS"] else {
            return false
        }
        switch rawValue.lowercased() {
        case "1", "true", "yes":
            return true
        default:
            return false
        }
    }()

    static func log(_ message: @autoclosure () -> String) {
        guard isEnabled else { return }
        let renderedMessage = message()
        logger.debug("\(renderedMessage, privacy: .public)")
        FileHandle.standardError.write(
            Data(("SRUI Renderer: " + renderedMessage + "\n").utf8)
        )
    }
}
