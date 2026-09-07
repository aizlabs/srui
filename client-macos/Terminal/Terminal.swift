//
// Terminal.swift
// Terminal
//
// AppKit-free Terminal compatibility types for `org.srui.terminal/1` (§21, §21.2).
// Normative requirement: this target must NEVER import AppKit or Cocoa.
//

import Foundation
import SemanticModel

/// Profile URI advertised in `ServerWelcome.extension_namespaces` and capability lists.
public let terminalProfileURI = "org.srui.terminal/1"

/// Local type ID of `Terminal` inside the session-assigned namespace. Clients must
/// never assume the numeric namespace is `1`.
public let terminalLocalTypeID: UInt32 = 1

public let maxTerminalInputBytes = 65_536
public let maxTerminalOutputFrameBytes = 16_384
public let maxTerminalResumeMapEntries = 256
public let maxTerminalColumns = 512
public let maxTerminalRows = 512
public let maxTerminalPixelDimension = 16_384

/// Builds the session-assigned `TypeRef` for a Terminal node.
public func terminalTypeRef(namespaceID: UInt32) -> TypeRef {
    TypeRef(namespaceID: namespaceID, localID: terminalLocalTypeID)
}

public enum TerminalColor: Equatable, Hashable, Sendable {
    case defaultForeground
    case defaultBackground
    case indexed(UInt8)
    case rgb(UInt8, UInt8, UInt8)
}

public struct TerminalAttributes: Equatable, Hashable, Sendable {
    public var bold: Bool
    public var faint: Bool
    public var italic: Bool
    public var underline: Bool
    public var inverse: Bool
    public var strikethrough: Bool
    public var blink: Bool
    public var foreground: TerminalColor
    public var background: TerminalColor
    public var hyperlink: String?

    public static let `default` = TerminalAttributes(
        bold: false,
        faint: false,
        italic: false,
        underline: false,
        inverse: false,
        strikethrough: false,
        blink: false,
        foreground: .defaultForeground,
        background: .defaultBackground,
        hyperlink: nil
    )

    public init(
        bold: Bool = false,
        faint: Bool = false,
        italic: Bool = false,
        underline: Bool = false,
        inverse: Bool = false,
        strikethrough: Bool = false,
        blink: Bool = false,
        foreground: TerminalColor = .defaultForeground,
        background: TerminalColor = .defaultBackground,
        hyperlink: String? = nil
    ) {
        self.bold = bold
        self.faint = faint
        self.italic = italic
        self.underline = underline
        self.inverse = inverse
        self.strikethrough = strikethrough
        self.blink = blink
        self.foreground = foreground
        self.background = background
        self.hyperlink = hyperlink
    }
}

public struct TerminalCell: Equatable, Hashable, Sendable {
    public var character: Character
    public var attributes: TerminalAttributes

    public static let empty = TerminalCell(character: " ", attributes: .default)

    public init(character: Character = " ", attributes: TerminalAttributes = .default) {
        self.character = character
        self.attributes = attributes
    }

    public var isEmpty: Bool {
        character == " " && attributes == .default
    }
}

public struct TerminalSnapshot: Equatable, Sendable {
    public var streamID: NodeId
    public var columns: Int
    public var rows: Int
    public var cursorColumn: Int
    public var cursorRow: Int
    public var cursorVisible: Bool
    public var cells: [[TerminalCell]]
    public var scrollback: [[TerminalCell]]
    public var title: String
    public var nextOffset: UInt64
    public var needsRedraw: Bool
    public var bracketedPaste: Bool
    public var applicationCursorKeys: Bool

    public init(
        streamID: NodeId,
        columns: Int,
        rows: Int,
        cursorColumn: Int,
        cursorRow: Int,
        cursorVisible: Bool,
        cells: [[TerminalCell]],
        scrollback: [[TerminalCell]] = [],
        title: String,
        nextOffset: UInt64,
        needsRedraw: Bool,
        bracketedPaste: Bool,
        applicationCursorKeys: Bool
    ) {
        self.streamID = streamID
        self.columns = columns
        self.rows = rows
        self.cursorColumn = cursorColumn
        self.cursorRow = cursorRow
        self.cursorVisible = cursorVisible
        self.cells = cells
        self.scrollback = scrollback
        self.title = title
        self.nextOffset = nextOffset
        self.needsRedraw = needsRedraw
        self.bracketedPaste = bracketedPaste
        self.applicationCursorKeys = applicationCursorKeys
    }

    public func plainText() -> String {
        cells.map { row in
            var chars = row.map(\.character)
            while let last = chars.last, last.isWhitespace {
                chars.removeLast()
            }
            return String(chars)
        }
        .joined(separator: "\n")
        .trimmingCharacters(in: CharacterSet.newlines.union(.whitespaces))
    }
}

public enum TerminalApplyError: Error, Equatable, Sendable {
    case emptyFrame
    case frameTooLarge(Int)
    case offsetOverflow
}

/// Mutable screen + parser state for one PTY stream. Not `Sendable`; owned by `TerminalSession`.
public final class TerminalGrid {
    public private(set) var columns: Int
    public private(set) var rows: Int
    public private(set) var cursorColumn: Int = 0
    public private(set) var cursorRow: Int = 0
    public var cursorVisible: Bool = true
    public var wraparound: Bool = true
    public var insertMode: Bool = false
    public var originMode: Bool = false
    public var applicationCursorKeys: Bool = false
    public var bracketedPaste: Bool = false
    public var title: String = ""
    public var currentAttributes: TerminalAttributes = .default
    public var pendingHyperlink: String?

    private var primary: [[TerminalCell]]
    public private(set) var scrollback: [[TerminalCell]] = []
    public var maxScrollbackLines: Int = 10_000

    private var alternate: [[TerminalCell]]?
    private var usingAlternate = false
    private var savedCursorColumn = 0
    private var savedCursorRow = 0
    private var savedAttributes: TerminalAttributes = .default
    private var scrollTop = 0
    private var scrollBottom: Int
    private var pendingWrap = false

    public init(columns: Int = 80, rows: Int = 24) {
        let cols = max(1, min(columns, maxTerminalColumns))
        let rws = max(1, min(rows, maxTerminalRows))
        self.columns = cols
        self.rows = rws
        self.primary = TerminalGrid.blankBuffer(columns: cols, rows: rws)
        self.scrollBottom = rws - 1
    }

    public var cells: [[TerminalCell]] {
        usingAlternate ? (alternate ?? primary) : primary
    }

    public func snapshot(streamID: NodeId, nextOffset: UInt64, needsRedraw: Bool) -> TerminalSnapshot {
        TerminalSnapshot(
            streamID: streamID,
            columns: columns,
            rows: rows,
            cursorColumn: cursorColumn,
            cursorRow: cursorRow,
            cursorVisible: cursorVisible,
            cells: cells,
            scrollback: scrollback,
            title: title,
            nextOffset: nextOffset,
            needsRedraw: needsRedraw,
            bracketedPaste: bracketedPaste,
            applicationCursorKeys: applicationCursorKeys
        )
    }

    public func reset() {
        columns = max(columns, 1)
        rows = max(rows, 1)
        primary = TerminalGrid.blankBuffer(columns: columns, rows: rows)
        alternate = nil
        scrollback.removeAll(keepingCapacity: true)
        usingAlternate = false
        cursorColumn = 0
        cursorRow = 0
        cursorVisible = true
        wraparound = true
        insertMode = false
        originMode = false
        applicationCursorKeys = false
        bracketedPaste = false
        title = ""
        currentAttributes = .default
        pendingHyperlink = nil
        savedCursorColumn = 0
        savedCursorRow = 0
        savedAttributes = .default
        scrollTop = 0
        scrollBottom = rows - 1
        pendingWrap = false
    }

    public func resize(columns newColumns: Int, rows newRows: Int) {
        let cols = max(1, min(newColumns, maxTerminalColumns))
        let rws = max(1, min(newRows, maxTerminalRows))
        primary = TerminalGrid.resized(primary, columns: cols, rows: rws)
        if let alt = alternate {
            alternate = TerminalGrid.resized(alt, columns: cols, rows: rws)
        }
        columns = cols
        rows = rws
        scrollTop = min(scrollTop, rws - 1)
        scrollBottom = rws - 1
        cursorColumn = min(cursorColumn, cols - 1)
        cursorRow = min(cursorRow, rws - 1)
        pendingWrap = false
    }

    public func put(_ scalar: Unicode.Scalar) {
        if scalar == "\u{7F}" { return }
        if pendingWrap && wraparound {
            carriageReturn()
            lineFeed()
            pendingWrap = false
        }
        var attrs = currentAttributes
        attrs.hyperlink = pendingHyperlink
        let character = Character(scalar)
        if insertMode {
            insertBlanks(1)
        }
        setCell(row: cursorRow, column: cursorColumn, TerminalCell(character: character, attributes: attrs))
        if cursorColumn + 1 >= columns {
            pendingWrap = wraparound
        } else {
            cursorColumn += 1
            pendingWrap = false
        }
    }

    public func backspace() {
        pendingWrap = false
        if cursorColumn > 0 {
            cursorColumn -= 1
        } else if cursorRow > originTop {
            cursorRow -= 1
            cursorColumn = columns - 1
        }
    }

    public func tab() {
        pendingWrap = false
        let next = ((cursorColumn / 8) + 1) * 8
        cursorColumn = min(next, columns - 1)
    }

    public func carriageReturn() {
        pendingWrap = false
        cursorColumn = 0
    }

    public func lineFeed() {
        pendingWrap = false
        if cursorRow == scrollBottom {
            scrollUp(1)
        } else if cursorRow + 1 < rows {
            cursorRow += 1
        }
    }

    public func reverseIndex() {
        pendingWrap = false
        if cursorRow == scrollTop {
            scrollDown(1)
        } else if cursorRow > 0 {
            cursorRow -= 1
        }
    }

    public func nextLine() {
        carriageReturn()
        lineFeed()
    }

    public func saveCursor() {
        savedCursorColumn = cursorColumn
        savedCursorRow = cursorRow
        savedAttributes = currentAttributes
    }

    public func restoreCursor() {
        cursorColumn = min(savedCursorColumn, columns - 1)
        cursorRow = min(savedCursorRow, rows - 1)
        currentAttributes = savedAttributes
        pendingWrap = false
    }

    public func moveCursor(row: Int, column: Int) {
        pendingWrap = false
        let top = originMode ? scrollTop : 0
        let bottom = originMode ? scrollBottom : rows - 1
        cursorRow = min(max(top + max(row, 0), top), bottom)
        cursorColumn = min(max(column, 0), columns - 1)
    }

    public func moveRelative(rows dRow: Int, columns dCol: Int) {
        pendingWrap = false
        cursorRow = min(max(cursorRow + dRow, originTop), originBottom)
        cursorColumn = min(max(cursorColumn + dCol, 0), columns - 1)
    }

    public func setCursorColumn(_ column: Int) {
        pendingWrap = false
        cursorColumn = min(max(column, 0), columns - 1)
    }

    public func setCursorRow(_ row: Int) {
        pendingWrap = false
        cursorRow = min(max(originTop + max(row, 0), originTop), originBottom)
    }

    public func eraseInDisplay(_ mode: Int) {
        switch mode {
        case 1:
            for row in 0..<cursorRow {
                fillRow(row, from: 0, count: columns)
            }
            fillRow(cursorRow, from: 0, count: cursorColumn + 1)
        case 2, 3:
            for row in 0..<rows {
                fillRow(row, from: 0, count: columns)
            }
        default:
            fillRow(cursorRow, from: cursorColumn, count: columns - cursorColumn)
            if cursorRow + 1 < rows {
                for row in (cursorRow + 1)..<rows {
                    fillRow(row, from: 0, count: columns)
                }
            }
        }
    }

    public func eraseInLine(_ mode: Int) {
        switch mode {
        case 1:
            fillRow(cursorRow, from: 0, count: cursorColumn + 1)
        case 2:
            fillRow(cursorRow, from: 0, count: columns)
        default:
            fillRow(cursorRow, from: cursorColumn, count: columns - cursorColumn)
        }
    }

    public func eraseCharacters(_ count: Int) {
        fillRow(cursorRow, from: cursorColumn, count: min(count, columns - cursorColumn))
    }

    public func insertCharacters(_ count: Int) {
        insertBlanks(count)
    }

    public func deleteCharacters(_ count: Int) {
        let row = cursorRow
        let start = cursorColumn
        let n = min(max(count, 0), columns - start)
        guard n > 0 else { return }
        withActiveBuffer { buf in
            var line = buf[row]
            line.removeSubrange(start..<(start + n))
            line.append(contentsOf: repeatElement(blankCell, count: n))
            buf[row] = line
        }
    }

    public func insertLines(_ count: Int) {
        guard cursorRow >= scrollTop && cursorRow <= scrollBottom else { return }
        let n = min(max(count, 0), scrollBottom - cursorRow + 1)
        guard n > 0 else { return }
        withActiveBuffer { buf in
            for _ in 0..<n {
                buf.remove(at: scrollBottom)
                buf.insert(Array(repeating: blankCell, count: columns), at: cursorRow)
            }
        }
    }

    public func deleteLines(_ count: Int) {
        guard cursorRow >= scrollTop && cursorRow <= scrollBottom else { return }
        let n = min(max(count, 0), scrollBottom - cursorRow + 1)
        guard n > 0 else { return }
        withActiveBuffer { buf in
            for _ in 0..<n {
                buf.remove(at: cursorRow)
                buf.insert(Array(repeating: blankCell, count: columns), at: scrollBottom)
            }
        }
    }

    public func setScrollRegion(top: Int, bottom: Int) {
        let t = min(max(top, 0), rows - 1)
        let b = min(max(bottom, t), rows - 1)
        scrollTop = t
        scrollBottom = b
        moveCursor(row: 0, column: 0)
    }

    public func scrollUp(_ count: Int) {
        let n = min(max(count, 0), scrollBottom - scrollTop + 1)
        withActiveBuffer { buf in
            for _ in 0..<n {
                let removed = buf.remove(at: scrollTop)
                if !usingAlternate {
                    scrollback.append(removed)
                    if scrollback.count > maxScrollbackLines {
                        scrollback.removeFirst(scrollback.count - maxScrollbackLines)
                    }
                }
                buf.insert(Array(repeating: blankCell, count: columns), at: scrollBottom)
            }
        }
    }

    public func scrollDown(_ count: Int) {
        let n = min(max(count, 0), scrollBottom - scrollTop + 1)
        withActiveBuffer { buf in
            for _ in 0..<n {
                buf.remove(at: scrollBottom)
                buf.insert(Array(repeating: blankCell, count: columns), at: scrollTop)
            }
        }
    }

    public func setAlternateScreen(_ enabled: Bool) {
        if enabled {
            if alternate == nil {
                alternate = TerminalGrid.blankBuffer(columns: columns, rows: rows)
            }
            usingAlternate = true
        } else {
            usingAlternate = false
        }
        pendingWrap = false
    }

    private var originTop: Int { originMode ? scrollTop : 0 }
    private var originBottom: Int { originMode ? scrollBottom : rows - 1 }

    private var blankCell: TerminalCell {
        TerminalCell(character: " ", attributes: currentAttributes)
    }

    @inline(__always)
    private func withActiveBuffer<T>(_ body: (inout [[TerminalCell]]) -> T) -> T {
        if usingAlternate {
            if alternate == nil {
                alternate = TerminalGrid.blankBuffer(columns: columns, rows: rows)
            }
            return body(&alternate!)
        } else {
            return body(&primary)
        }
    }

    @inline(__always)
    private func setCell(row: Int, column: Int, _ cell: TerminalCell) {
        if usingAlternate {
            if alternate == nil {
                alternate = TerminalGrid.blankBuffer(columns: columns, rows: rows)
            }
            alternate![row][column] = cell
        } else {
            primary[row][column] = cell
        }
    }

    private func insertBlanks(_ count: Int) {
        let n = min(max(count, 0), columns - cursorColumn)
        guard n > 0 else { return }
        withActiveBuffer { buf in
            var line = buf[cursorRow]
            line.insert(contentsOf: repeatElement(blankCell, count: n), at: cursorColumn)
            if line.count > columns {
                line.removeLast(line.count - columns)
            }
            buf[cursorRow] = line
        }
    }

    private func fillRow(_ row: Int, from start: Int, count: Int) {
        guard row >= 0, row < rows else { return }
        let end = min(columns, start + max(count, 0))
        guard start < end else { return }
        for col in start..<end {
            setCell(row: row, column: col, blankCell)
        }
    }

    private static func blankBuffer(columns: Int, rows: Int) -> [[TerminalCell]] {
        Array(
            repeating: Array(repeating: TerminalCell.empty, count: columns),
            count: rows
        )
    }

    private static func resized(_ source: [[TerminalCell]], columns: Int, rows: Int) -> [[TerminalCell]] {
        var result: [[TerminalCell]] = []
        result.reserveCapacity(rows)
        for row in 0..<rows {
            if row < source.count {
                var line = source[row]
                if line.count < columns {
                    line.append(contentsOf: repeatElement(TerminalCell.empty, count: columns - line.count))
                } else if line.count > columns {
                    line.removeLast(line.count - columns)
                }
                result.append(line)
            } else {
                result.append(Array(repeating: TerminalCell.empty, count: columns))
            }
        }
        return result
    }
}
