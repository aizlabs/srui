//
// TerminalView.swift
// RendererAppKit
//
// Native AppKit view for an `org.srui.terminal/1` island (§21, §22).
//

import AppKit
import SemanticModel
import Terminal

/// Monospace PTY surface. Local echo is forbidden: glyphs come only from `TerminalSnapshot`.
@MainActor
public final class TerminalView: NSView {
    public static let conventionalColumns: UInt32 = 80
    public static let conventionalRows: UInt32 = 24

    public let nodeID: NodeId
    public var onInput: ((Data) -> Void)?
    public var onResize: ((UInt32, UInt32, UInt32, UInt32) -> Void)?
    public var onAcknowledgeRedraw: (() -> Void)?
    public var snapshotSubscriptionTask: Task<Void, Never>?

    private var snapshot: TerminalSnapshot?
    private var font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    private var cellSize = NSSize(width: 8, height: 16)
    private var selection: Range<Int>?
    fileprivate var markedText = ""
    private var resizeWork: DispatchWorkItem?
    private var lastReportedSize: (UInt32, UInt32, UInt32, UInt32)?

    public init(nodeID: NodeId) {
        self.nodeID = nodeID
        super.init(frame: NSRect(x: 0, y: 0, width: 640, height: 384))
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        layer?.masksToBounds = true
        setAccessibilityRole(.textArea)
        setAccessibilityLabel("Terminal")
        setAccessibilityElement(true)
        measureCells()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    deinit {
        snapshotSubscriptionTask?.cancel()
    }

    public override func viewWillMove(toSuperview newSuperview: NSView?) {
        super.viewWillMove(toSuperview: newSuperview)
        if newSuperview == nil {
            resizeWork?.cancel()
            snapshotSubscriptionTask?.cancel()
            snapshotSubscriptionTask = nil
        }
    }

    public func apply(_ snapshot: TerminalSnapshot) {
        self.snapshot = snapshot
        setAccessibilityValue(snapshot.plainText())
        needsDisplay = true
    }

    public override var acceptsFirstResponder: Bool { true }
    public override var isFlipped: Bool { true }
    /// A terminal has no AppKit-provided intrinsic size. Prefer the conventional 80×24
    /// terminal geometry while allowing semantic `shrink` to compress it on smaller displays.
    /// Stack-layout growth priorities allocate any additional space to the terminal.
    public override var intrinsicContentSize: NSSize {
        NSSize(
            width: cellSize.width * CGFloat(Self.conventionalColumns),
            height: cellSize.height * CGFloat(Self.conventionalRows)
        )
    }
    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        scheduleResize()
    }

    public override func layout() {
        super.layout()
        scheduleResize()
    }

    public override func draw(_ dirtyRect: NSRect) {
        let paintRect = clippedDirtyRect(dirtyRect)
        guard !paintRect.isNull, !paintRect.isEmpty else { return }
        NSColor.black.setFill()
        paintRect.fill()
        guard let snapshot else { return }
        let attrs = defaultDrawingAttributes()
        for (row, line) in snapshot.cells.enumerated() {
            for (col, cell) in line.enumerated() {
                let rect = cellRect(column: col, row: row)
                if !rect.intersects(paintRect) { continue }
                var fg = color(for: cell.attributes.foreground, fallback: .white)
                var bg = color(for: cell.attributes.background, fallback: .black)
                if cell.attributes.inverse { swap(&fg, &bg) }
                bg.setFill()
                rect.fill()
                if cell.character != " " {
                    let string = String(cell.character) as NSString
                    var draw = attrs
                    draw[.foregroundColor] = fg
                    if cell.attributes.bold {
                        draw[.font] = NSFont.monospacedSystemFont(ofSize: font.pointSize, weight: .bold)
                    }
                    if cell.attributes.underline {
                        draw[.underlineStyle] = NSUnderlineStyle.single.rawValue
                    }
                    if cell.attributes.strikethrough {
                        draw[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
                    }
                    string.draw(in: rect, withAttributes: draw)
                }
            }
        }
        if snapshot.cursorVisible {
            let cursor = cellRect(column: snapshot.cursorColumn, row: snapshot.cursorRow)
            NSColor.white.withAlphaComponent(0.35).setFill()
            cursor.fill()
        }
        if snapshot.needsRedraw {
            let notice = " [Desynchronized — press any key to refresh] " as NSString
            let noticeAttrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .semibold),
                .foregroundColor: NSColor.black,
                .backgroundColor: NSColor.systemYellow,
            ]
            let noticeSize = notice.size(withAttributes: noticeAttrs)
            let noticeRect = NSRect(
                x: max(0, bounds.width - noticeSize.width - 8),
                y: 4,
                width: noticeSize.width,
                height: noticeSize.height
            )
            notice.draw(in: noticeRect, withAttributes: noticeAttrs)
        }
    }

    public override func keyDown(with event: NSEvent) {
        if snapshot?.needsRedraw == true {
            onAcknowledgeRedraw?()
        }
        if !markedText.isEmpty {
            interpretKeyEvents([event])
            return
        }
        if let special = Self.specialKey(from: event) {
            emit(TerminalInputEncoder.encode(
                key: special,
                applicationCursorKeys: snapshot?.applicationCursorKeys ?? false
            ))
            return
        }
        if event.modifierFlags.contains(.control),
           let chars = event.characters,
           let scalar = chars.unicodeScalars.first,
           scalar.value < 0x20 || scalar.value == 0x7F {
            emit(Data([UInt8(scalar.value)]))
            return
        }
        interpretKeyEvents([event])
    }

    public override func doCommand(by selector: Selector) {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            emit(TerminalInputEncoder.encode(key: .enter))
        case #selector(NSResponder.insertTab(_:)):
            emit(TerminalInputEncoder.encode(key: .tab))
        case #selector(NSResponder.cancelOperation(_:)):
            emit(TerminalInputEncoder.encode(key: .escape))
        case #selector(NSResponder.deleteBackward(_:)):
            emit(TerminalInputEncoder.encode(key: .backspace))
        case #selector(NSResponder.deleteForward(_:)):
            emit(TerminalInputEncoder.encode(key: .delete))
        case #selector(NSResponder.moveUp(_:)):
            emit(TerminalInputEncoder.encode(key: .arrowUp, applicationCursorKeys: snapshot?.applicationCursorKeys ?? false))
        case #selector(NSResponder.moveDown(_:)):
            emit(TerminalInputEncoder.encode(key: .arrowDown, applicationCursorKeys: snapshot?.applicationCursorKeys ?? false))
        case #selector(NSResponder.moveLeft(_:)):
            emit(TerminalInputEncoder.encode(key: .arrowLeft, applicationCursorKeys: snapshot?.applicationCursorKeys ?? false))
        case #selector(NSResponder.moveRight(_:)):
            emit(TerminalInputEncoder.encode(key: .arrowRight, applicationCursorKeys: snapshot?.applicationCursorKeys ?? false))
        case #selector(NSResponder.pageUp(_:)), #selector(NSResponder.scrollPageUp(_:)):
            emit(TerminalInputEncoder.encode(key: .pageUp))
        case #selector(NSResponder.pageDown(_:)), #selector(NSResponder.scrollPageDown(_:)):
            emit(TerminalInputEncoder.encode(key: .pageDown))
        case #selector(NSResponder.moveToBeginningOfLine(_:)):
            emit(Data([0x01]))
        case #selector(NSResponder.moveToEndOfLine(_:)):
            emit(Data([0x05]))
        case #selector(NSResponder.deleteToEndOfParagraph(_:)):
            emit(Data([0x0B]))
        case #selector(NSResponder.scrollToBeginningOfDocument(_:)):
            emit(TerminalInputEncoder.encode(key: .home))
        case #selector(NSResponder.scrollToEndOfDocument(_:)):
            emit(TerminalInputEncoder.encode(key: .end))
        default:
            super.doCommand(by: selector)
        }
    }

    @objc public func copy(_ sender: Any?) {
        let text = snapshot?.plainText() ?? ""
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @objc public func paste(_ sender: Any?) {
        guard let text = NSPasteboard.general.string(forType: .string), !text.isEmpty else { return }
        emit(TerminalInputEncoder.encodePaste(text, bracketed: snapshot?.bracketedPaste ?? false))
    }

    public override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    public override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        super.mouseDown(with: event)
    }

    fileprivate func emit(_ data: Data) {
        guard !data.isEmpty else { return }
        onInput?(data)
    }

    private func scheduleResize() {
        resizeWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            self?.publishResize()
        }
        resizeWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05, execute: work)
    }

    private func publishResize() {
        measureCells()
        guard !bounds.width.isNaN, !bounds.height.isNaN, bounds.width > 0, bounds.height > 0 else { return }
        let cellW = max(cellSize.width, 1)
        let cellH = max(cellSize.height, 1)
        let colsFloat = max(1.0, min(floor(bounds.width / cellW), Double(maxTerminalColumns)))
        let rowsFloat = max(1.0, min(floor(bounds.height / cellH), Double(maxTerminalRows)))
        let cols = UInt32(colsFloat)
        let rows = UInt32(rowsFloat)
        let pixelsW = UInt32(max(0.0, min(bounds.width, Double(maxTerminalPixelDimension))))
        let pixelsH = UInt32(max(0.0, min(bounds.height, Double(maxTerminalPixelDimension))))
        let size = (cols, rows, pixelsW, pixelsH)
        if lastReportedSize == nil || lastReportedSize! != size {
            lastReportedSize = size
            onResize?(cols, rows, pixelsW, pixelsH)
        }
    }

    private func measureCells() {
        let advance = ("M" as NSString).size(withAttributes: [.font: font])
        cellSize = NSSize(width: max(advance.width, 1), height: max(font.boundingRectForFont.height, font.pointSize + 4))
    }

    func clippedDirtyRect(_ dirtyRect: NSRect) -> NSRect {
        bounds.intersection(dirtyRect)
    }

    private func cellRect(column: Int, row: Int) -> NSRect {
        NSRect(
            x: CGFloat(column) * cellSize.width,
            y: CGFloat(row) * cellSize.height,
            width: cellSize.width,
            height: cellSize.height
        )
    }

    private func defaultDrawingAttributes() -> [NSAttributedString.Key: Any] {
        [.font: font, .foregroundColor: NSColor.white]
    }

    private func color(for terminal: TerminalColor, fallback: NSColor) -> NSColor {
        switch terminal {
        case .defaultForeground:
            return .white
        case .defaultBackground:
            return .black
        case .indexed(let index):
            return Self.xtermColor(index) ?? fallback
        case .rgb(let r, let g, let b):
            return NSColor(srgbRed: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: 1)
        }
    }

    private static func specialKey(from event: NSEvent) -> TerminalSpecialKey? {
        switch event.keyCode {
        case 36, 76: return .enter
        case 51: return .backspace
        case 48: return .tab
        case 53: return .escape
        case 126: return .arrowUp
        case 125: return .arrowDown
        case 124: return .arrowRight
        case 123: return .arrowLeft
        case 115: return .home
        case 119: return .end
        case 116: return .pageUp
        case 121: return .pageDown
        case 117: return .delete
        case 114: return .insert
        default:
            if let chars = event.charactersIgnoringModifiers, chars.count == 1,
               let scalar = chars.unicodeScalars.first {
                let fIndex = Int(scalar.value) - 0xF704 + 1
                if (1...12).contains(fIndex) {
                    return .function(fIndex)
                }
            }
            return nil
        }
    }

    private static func xtermColor(_ index: UInt8) -> NSColor? {
        let palette: [NSColor] = [
            .black, .systemRed, .systemGreen, .systemYellow,
            .systemBlue, .systemPurple, .systemTeal, .white,
        ]
        if index < 8 { return palette[Int(index)] }
        if index < 16 { return palette[Int(index - 8)].highlight(withLevel: 0.35) ?? palette[Int(index - 8)] }
        return nil
    }
}

extension TerminalView: @preconcurrency NSTextInputClient {
    public func insertText(_ string: Any, replacementRange: NSRange) {
        markedText = ""
        let text: String
        if let value = string as? String {
            text = value
        } else if let value = string as? NSAttributedString {
            text = value.string
        } else {
            return
        }
        emit(TerminalInputEncoder.encode(text: text))
    }

    public func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        if let value = string as? String {
            markedText = value
        } else if let value = string as? NSAttributedString {
            markedText = value.string
        } else {
            markedText = ""
        }
    }

    public func unmarkText() {
        markedText = ""
    }

    public func selectedRange() -> NSRange { NSRange(location: 0, length: 0) }
    public func markedRange() -> NSRange {
        markedText.isEmpty ? NSRange(location: NSNotFound, length: 0) : NSRange(location: 0, length: markedText.utf16.count)
    }
    public func hasMarkedText() -> Bool { !markedText.isEmpty }
    public func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? { nil }
    public func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }
    public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        window?.convertToScreen(convert(bounds, to: nil)) ?? .zero
    }
    public func characterIndex(for point: NSPoint) -> Int { 0 }
}
