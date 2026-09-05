//
// VTParser.swift
// Terminal
//
// Byte-oriented VT/xterm parser driving `TerminalGrid` (§21).
// Normative requirement: this target must NEVER import AppKit or Cocoa.
//

import Foundation

/// Incremental parser for PTY output. Does not construct semantic widgets from bytes.
public struct VTParser: Sendable {
    public static let maxOSCBytes = 4096
    public static let maxCSIBytes = 256

    private enum State: Sendable {
        case ground
        case escape
        case csi
        case osc
        case ignoreUntilST
    }

    private var state: State = .ground
    private var osc: [UInt8] = []
    private var csi: [UInt8] = []
    private var utf8Need = 0
    private var utf8Scalar: UInt32 = 0

    public init() {}

    public mutating func reset() {
        state = .ground
        osc.removeAll(keepingCapacity: true)
        csi.removeAll(keepingCapacity: true)
        utf8Need = 0
        utf8Scalar = 0
    }

    public mutating func feed(_ data: Data, into grid: TerminalGrid) {
        for byte in data {
            consume(byte, grid: grid)
        }
    }

    private mutating func consume(_ byte: UInt8, grid: TerminalGrid) {
        if utf8Need > 0 {
            if byte & 0xC0 == 0x80 {
                utf8Scalar = (utf8Scalar << 6) | UInt32(byte & 0x3F)
                utf8Need -= 1
                if utf8Need == 0, let scalar = Unicode.Scalar(utf8Scalar) {
                    grid.put(scalar)
                }
            } else {
                utf8Need = 0
                consume(byte, grid: grid)
            }
            return
        }

        switch state {
        case .ground:
            handleGround(byte, grid: grid)
        case .escape:
            handleEscape(byte, grid: grid)
        case .csi:
            handleCSI(byte, grid: grid)
        case .osc:
            handleOSC(byte, grid: grid)
        case .ignoreUntilST:
            if byte == 0x1B {
                state = .escape
            } else if byte == 0x07 {
                state = .ground
            }
        }
    }

    private mutating func handleGround(_ byte: UInt8, grid: TerminalGrid) {
        switch byte {
        case 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0E, 0x0F:
            return
        case 0x07:
            return
        case 0x08:
            grid.backspace()
        case 0x09:
            grid.tab()
        case 0x0A, 0x0B, 0x0C:
            grid.lineFeed()
        case 0x0D:
            grid.carriageReturn()
        case 0x18, 0x1A:
            state = .ground
        case 0x1B:
            state = .escape
        case 0x20...0x7E:
            grid.put(Unicode.Scalar(byte))
        case 0x7F:
            return
        case 0xC2...0xDF:
            utf8Need = 1
            utf8Scalar = UInt32(byte & 0x1F)
        case 0xE0...0xEF:
            utf8Need = 2
            utf8Scalar = UInt32(byte & 0x0F)
        case 0xF0...0xF4:
            utf8Need = 3
            utf8Scalar = UInt32(byte & 0x07)
        default:
            return
        }
    }

    private mutating func handleEscape(_ byte: UInt8, grid: TerminalGrid) {
        switch byte {
        case 0x1B:
            return
        case 0x5B: // [
            csi.removeAll(keepingCapacity: true)
            state = .csi
        case 0x5D: // ]
            osc.removeAll(keepingCapacity: true)
            state = .osc
        case 0x50, 0x5E, 0x5F, 0x58: // P ^ _ X
            state = .ignoreUntilST
        case 0x37: // 7
            grid.saveCursor()
            state = .ground
        case 0x38: // 8
            grid.restoreCursor()
            state = .ground
        case 0x63: // c
            grid.reset()
            reset()
        case 0x44: // D
            grid.lineFeed()
            state = .ground
        case 0x45: // E
            grid.nextLine()
            state = .ground
        case 0x4D: // M
            grid.reverseIndex()
            state = .ground
        case 0x28, 0x29, 0x2A, 0x2B:
            // charset designate; consume next byte in ground on next call via ignore-one
            state = .ignoreUntilST
            // treat the next graphic as the designator then return to ground
            state = .ground
            _ = byte
        default:
            state = .ground
        }
    }

    private mutating func handleCSI(_ byte: UInt8, grid: TerminalGrid) {
        if csi.count >= Self.maxCSIBytes {
            state = .ground
            return
        }
        if (0x40...0x7E).contains(byte) {
            csi.append(byte)
            applyCSI(csi, grid: grid)
            csi.removeAll(keepingCapacity: true)
            state = .ground
            return
        }
        if (0x20...0x3F).contains(byte) {
            csi.append(byte)
            return
        }
        if byte == 0x1B {
            state = .escape
            csi.removeAll(keepingCapacity: true)
            return
        }
        state = .ground
        csi.removeAll(keepingCapacity: true)
    }

    private mutating func handleOSC(_ byte: UInt8, grid: TerminalGrid) {
        if byte == 0x07 {
            applyOSC(osc, grid: grid)
            osc.removeAll(keepingCapacity: true)
            state = .ground
            return
        }
        if byte == 0x1B {
            state = .escape
            // BEL-less ST is ESC \ ; stash and finish if next is '\'
            applyOSC(osc, grid: grid)
            osc.removeAll(keepingCapacity: true)
            state = .ground
            return
        }
        if osc.count >= Self.maxOSCBytes {
            osc.removeAll(keepingCapacity: true)
            state = .ignoreUntilST
            return
        }
        osc.append(byte)
    }

    private func applyCSI(_ raw: [UInt8], grid: TerminalGrid) {
        guard let final = raw.last else { return }
        let body = raw.dropLast()
        let privateMark = body.first == UInt8(ascii: "?")
        let paramsSource = privateMark ? body.dropFirst() : body
        let intermediates = paramsSource.filter { (0x20...0x2F).contains($0) }
        _ = intermediates
        let paramBytes = paramsSource.filter { $0 == UInt8(ascii: ";") || (0x30...0x39).contains($0) }
        let params = parseParams(paramBytes)

        if privateMark {
            applyPrivate(final: final, params: params, grid: grid)
            return
        }

        let p1 = params.first ?? 0
        switch final {
        case UInt8(ascii: "A"):
            grid.moveRelative(rows: -max(p1 == 0 ? 1 : p1, 1), columns: 0)
        case UInt8(ascii: "B"):
            grid.moveRelative(rows: max(p1 == 0 ? 1 : p1, 1), columns: 0)
        case UInt8(ascii: "C"):
            grid.moveRelative(rows: 0, columns: max(p1 == 0 ? 1 : p1, 1))
        case UInt8(ascii: "D"):
            grid.moveRelative(rows: 0, columns: -max(p1 == 0 ? 1 : p1, 1))
        case UInt8(ascii: "H"), UInt8(ascii: "f"):
            let row = (params.indices.contains(0) ? params[0] : 1) - 1
            let col = (params.indices.contains(1) ? params[1] : 1) - 1
            grid.moveCursor(row: max(row, 0), column: max(col, 0))
        case UInt8(ascii: "J"):
            grid.eraseInDisplay(p1)
        case UInt8(ascii: "K"):
            grid.eraseInLine(p1)
        case UInt8(ascii: "L"):
            grid.insertLines(p1 == 0 ? 1 : p1)
        case UInt8(ascii: "M"):
            grid.deleteLines(p1 == 0 ? 1 : p1)
        case UInt8(ascii: "P"):
            grid.deleteCharacters(p1 == 0 ? 1 : p1)
        case UInt8(ascii: "@"):
            grid.insertCharacters(p1 == 0 ? 1 : p1)
        case UInt8(ascii: "X"):
            grid.eraseCharacters(p1 == 0 ? 1 : p1)
        case UInt8(ascii: "G"):
            grid.setCursorColumn(max((p1 == 0 ? 1 : p1) - 1, 0))
        case UInt8(ascii: "d"):
            grid.setCursorRow(max((p1 == 0 ? 1 : p1) - 1, 0))
        case UInt8(ascii: "r"):
            let top = (params.indices.contains(0) ? params[0] : 1) - 1
            let bottom = (params.indices.contains(1) ? params[1] : grid.rows) - 1
            grid.setScrollRegion(top: max(top, 0), bottom: max(bottom, 0))
        case UInt8(ascii: "S"):
            grid.scrollUp(p1 == 0 ? 1 : p1)
        case UInt8(ascii: "T"):
            grid.scrollDown(p1 == 0 ? 1 : p1)
        case UInt8(ascii: "m"):
            applySGR(params.isEmpty ? [0] : params, grid: grid)
        case UInt8(ascii: "h"):
            if params.contains(4) { grid.insertMode = true }
        case UInt8(ascii: "l"):
            if params.contains(4) { grid.insertMode = false }
        default:
            break
        }
    }

    private func applyPrivate(final: UInt8, params: [Int], grid: TerminalGrid) {
        let enable = final == UInt8(ascii: "h")
        for mode in params {
            switch mode {
            case 1:
                grid.applicationCursorKeys = enable
            case 6:
                grid.originMode = enable
            case 7:
                grid.wraparound = enable
            case 25:
                grid.cursorVisible = enable
            case 2004:
                grid.bracketedPaste = enable
            case 1047, 1049:
                if enable {
                    if mode == 1049 { grid.saveCursor() }
                    grid.setAlternateScreen(true)
                } else {
                    grid.setAlternateScreen(false)
                    if mode == 1049 { grid.restoreCursor() }
                }
            default:
                break
            }
        }
    }

    private func applySGR(_ params: [Int], grid: TerminalGrid) {
        var i = 0
        var attrs = grid.currentAttributes
        while i < params.count {
            let code = params[i]
            switch code {
            case 0:
                let link = attrs.hyperlink
                attrs = .default
                attrs.hyperlink = link
            case 1: attrs.bold = true
            case 2: attrs.faint = true
            case 3: attrs.italic = true
            case 4: attrs.underline = true
            case 5, 6: attrs.blink = true
            case 7: attrs.inverse = true
            case 9: attrs.strikethrough = true
            case 22:
                attrs.bold = false
                attrs.faint = false
            case 23: attrs.italic = false
            case 24: attrs.underline = false
            case 25: attrs.blink = false
            case 27: attrs.inverse = false
            case 29: attrs.strikethrough = false
            case 30...37:
                attrs.foreground = .indexed(UInt8(code - 30))
            case 39:
                attrs.foreground = .defaultForeground
            case 40...47:
                attrs.background = .indexed(UInt8(code - 40))
            case 49:
                attrs.background = .defaultBackground
            case 90...97:
                attrs.foreground = .indexed(UInt8(code - 90 + 8))
            case 100...107:
                attrs.background = .indexed(UInt8(code - 100 + 8))
            case 38, 48:
                let isFg = code == 38
                if i + 1 < params.count, params[i + 1] == 5, i + 2 < params.count {
                    let color = TerminalColor.indexed(UInt8(clamping: params[i + 2]))
                    if isFg { attrs.foreground = color } else { attrs.background = color }
                    i += 2
                } else if i + 1 < params.count, params[i + 1] == 2, i + 4 < params.count {
                    let color = TerminalColor.rgb(
                        UInt8(clamping: params[i + 2]),
                        UInt8(clamping: params[i + 3]),
                        UInt8(clamping: params[i + 4])
                    )
                    if isFg { attrs.foreground = color } else { attrs.background = color }
                    i += 4
                }
            default:
                break
            }
            i += 1
        }
        grid.currentAttributes = attrs
    }

    private func applyOSC(_ raw: [UInt8], grid: TerminalGrid) {
        guard let semicolon = raw.firstIndex(of: UInt8(ascii: ";")) else { return }
        let code = String(bytes: raw[..<semicolon], encoding: .ascii) ?? ""
        let payload = Array(raw[(semicolon + 1)...])
        switch code {
        case "0", "2":
            if let title = String(bytes: payload, encoding: .utf8) {
                grid.title = title
            }
        case "8":
            // OSC 8 ;;uri ST  or OSC 8 ;id;uri ST. Empty URI closes the link.
            let parts = splitOSC8(payload)
            let uri = parts.last ?? ""
            grid.pendingHyperlink = uri.isEmpty ? nil : uri
        case "52":
            // Clipboard OSC is ignored: the client never writes the pasteboard from PTY bytes.
            return
        default:
            return
        }
    }

    private func splitOSC8(_ payload: [UInt8]) -> [String] {
        payload.split(separator: UInt8(ascii: ";"), omittingEmptySubsequences: false).map {
            String(bytes: $0, encoding: .utf8) ?? ""
        }
    }

    private func parseParams(_ bytes: [UInt8]) -> [Int] {
        if bytes.isEmpty { return [] }
        var values: [Int] = []
        var current = 0
        var sawDigit = false
        for byte in bytes {
            if byte == UInt8(ascii: ";") {
                values.append(sawDigit ? current : 0)
                current = 0
                sawDigit = false
            } else if let digit = Int(exactly: byte - UInt8(ascii: "0")), (0...9).contains(digit) {
                current = min(current * 10 + digit, 9999)
                sawDigit = true
            }
        }
        values.append(sawDigit ? current : 0)
        return values
    }
}
