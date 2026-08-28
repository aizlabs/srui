//
// SessionDiagnostics.swift
// Session
//
// Opt-in session coordinator diagnostics (§22).
//

import Foundation
import OSLog

/// Session-layer diagnostics. Errors always go to stderr; info logs honor `SRUI_SESSION_LOGS`.
///
/// Writes go through `fputs` rather than `FileHandle.standardError.write(_:)`: the latter raises an
/// uncatchable Objective-C exception on `EPIPE`/`EBADF`, which would turn a closed stderr (an
/// `srui-ssh-bridge` or daemonized host) into a crash on what is now an always-on error path.
enum SessionDiagnostics {
    private static let logger = Logger(subsystem: "org.srui.Session", category: "session")
    private static let stderrLock = NSLock()

    private static func emit(_ text: String) {
        stderrLock.lock()
        defer { stderrLock.unlock() }
        fputs(text, stderr)
        fflush(stderr)
    }

    private static let isInfoEnabled: Bool = {
        guard let rawValue = ProcessInfo.processInfo.environment["SRUI_SESSION_LOGS"] else {
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
        guard isInfoEnabled else { return }
        let rendered = message()
        logger.debug("\(rendered, privacy: .public)")
        emit("SRUI Session: " + rendered + "\n")
    }

    static func error(_ message: @autoclosure () -> String) {
        let rendered = message()
        logger.error("\(rendered, privacy: .public)")
        emit("SRUI Session: " + rendered + "\n")
    }
}
