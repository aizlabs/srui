//
// SessionDiagnostics.swift
// Session
//
// Opt-in session coordinator diagnostics (§22).
//

import Foundation

/// Session-layer diagnostics. Errors always go to stderr; info logs honor `SRUI_SESSION_LOGS`.
enum SessionDiagnostics {
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
        FileHandle.standardError.write(
            Data(("SRUI Session: " + message() + "\n").utf8)
        )
    }

    static func error(_ message: @autoclosure () -> String) {
        FileHandle.standardError.write(
            Data(("SRUI Session: " + message() + "\n").utf8)
        )
    }
}
