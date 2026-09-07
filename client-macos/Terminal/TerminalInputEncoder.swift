//
// TerminalInputEncoder.swift
// Terminal
//
// Encodes keystrokes and paste into PTY bytes (§21).
// Normative requirement: this target must NEVER import AppKit or Cocoa.
//

import Foundation

public enum TerminalSpecialKey: Equatable, Sendable {
    case enter
    case backspace
    case tab
    case escape
    case arrowUp
    case arrowDown
    case arrowRight
    case arrowLeft
    case home
    case end
    case pageUp
    case pageDown
    case delete
    case insert
    case function(Int)
}

public enum TerminalInputEncoder {
    /// Encodes a Unicode scalar typed by the user. Control characters pass through.
    public static func encode(scalar: Unicode.Scalar, applicationCursorKeys: Bool = false) -> Data {
        _ = applicationCursorKeys
        return Data(String(scalar).utf8)
    }

    public static func encode(text: String) -> Data {
        Data(text.utf8)
    }

    public static func encode(key: TerminalSpecialKey, applicationCursorKeys: Bool = false) -> Data {
        switch key {
        case .enter:
            return Data([0x0D])
        case .backspace:
            return Data([0x7F])
        case .tab:
            return Data([0x09])
        case .escape:
            return Data([0x1B])
        case .arrowUp:
            return Data(applicationCursorKeys ? [0x1B, 0x4F, 0x41] : [0x1B, 0x5B, 0x41])
        case .arrowDown:
            return Data(applicationCursorKeys ? [0x1B, 0x4F, 0x42] : [0x1B, 0x5B, 0x42])
        case .arrowRight:
            return Data(applicationCursorKeys ? [0x1B, 0x4F, 0x43] : [0x1B, 0x5B, 0x43])
        case .arrowLeft:
            return Data(applicationCursorKeys ? [0x1B, 0x4F, 0x44] : [0x1B, 0x5B, 0x44])
        case .home:
            return Data([0x1B, 0x5B, 0x48])
        case .end:
            return Data([0x1B, 0x5B, 0x46])
        case .pageUp:
            return Data([0x1B, 0x5B, 0x35, 0x7E])
        case .pageDown:
            return Data([0x1B, 0x5B, 0x36, 0x7E])
        case .delete:
            return Data([0x1B, 0x5B, 0x33, 0x7E])
        case .insert:
            return Data([0x1B, 0x5B, 0x32, 0x7E])
        case .function(let number):
            return functionKey(number)
        }
    }

    /// Wraps clipboard text. When `bracketed` is true the paste is framed with OSC-equivalent
    /// CSI 200~ / 201~ so the remote application can distinguish it from typed input.
    public static func encodePaste(_ text: String, bracketed: Bool) -> Data {
        if text.isEmpty {
            return Data()
        }
        if !bracketed {
            return Data(text.utf8)
        }
        // Strip embedded bracketed paste end markers (ESC [ 201 ~) to prevent pastejacking attacks
        let sanitized = text.replacingOccurrences(of: "\u{1B}[201~", with: "")
        var framed = Data([0x1B, 0x5B, 0x32, 0x30, 0x30, 0x7E])
        framed.append(contentsOf: sanitized.utf8)
        framed.append(contentsOf: [0x1B, 0x5B, 0x32, 0x30, 0x31, 0x7E])
        return framed
    }

    private static func functionKey(_ number: Int) -> Data {
        let seq: [UInt8]
        switch number {
        case 1: seq = [0x1B, 0x4F, 0x50]
        case 2: seq = [0x1B, 0x4F, 0x51]
        case 3: seq = [0x1B, 0x4F, 0x52]
        case 4: seq = [0x1B, 0x4F, 0x53]
        case 5: seq = [0x1B, 0x5B, 0x31, 0x35, 0x7E]
        case 6: seq = [0x1B, 0x5B, 0x31, 0x37, 0x7E]
        case 7: seq = [0x1B, 0x5B, 0x31, 0x38, 0x7E]
        case 8: seq = [0x1B, 0x5B, 0x31, 0x39, 0x7E]
        case 9: seq = [0x1B, 0x5B, 0x32, 0x30, 0x7E]
        case 10: seq = [0x1B, 0x5B, 0x32, 0x31, 0x7E]
        case 11: seq = [0x1B, 0x5B, 0x32, 0x33, 0x7E]
        case 12: seq = [0x1B, 0x5B, 0x32, 0x34, 0x7E]
        default:
            return Data()
        }
        return Data(seq)
    }
}
