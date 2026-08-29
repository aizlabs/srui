import AppKit
import SemanticModel

public enum ControlFactoryError: Error, Equatable, Sendable {
    case unsupportedNodeType(TypeRef)
}

/// Target-action trampoline for interactive AppKit controls (§7.6, §7.7, §22).
@MainActor
public final class ActionTrampoline: NSObject {
    public let nodeID: NodeId
    public let eventType: TypeRef
    public let handler: @MainActor (NodeId, TypeRef) -> Void

    public init(
        nodeID: NodeId,
        eventType: TypeRef = .EVENT_ACTIVATE,
        handler: @escaping @MainActor (NodeId, TypeRef) -> Void
    ) {
        self.nodeID = nodeID
        self.eventType = eventType
        self.handler = handler
    }

    @objc public func performAction(_ sender: Any?) {
        handler(nodeID, eventType)
    }
}

/// Creates native controls for the required §7.3 tier and applies scalar properties in place.
@MainActor
public final class ControlFactory {
    /// Semantic action callback invoked when a native interactive control is activated (§7.6, §7.7).
    public var onAction: (@MainActor (NodeId, TypeRef) -> Void)?

    public init() {}

    public func makeHandle(for node: Node) throws -> RenderHandle {
        let result: (view: NSView, window: NSWindow?, adapter: AnyObject?, trampoline: AnyObject?)

        switch node.nodeType {
        case .surface:
            let contentView = NSStackView(frame: NSRect(x: 0, y: 0, width: 440, height: 320))
            contentView.orientation = .vertical
            contentView.alignment = .leading
            contentView.distribution = .fill
            contentView.spacing = 14
            contentView.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 440, height: 320),
                styleMask: [.titled, .closable, .miniaturizable, .resizable],
                backing: .buffered,
                defer: false
            )
            // RenderHandle keeps a strong reference and LayoutRenderer.tearDown() closes the
            // window on remount; AppKit's default would then release it a second time.
            window.isReleasedWhenClosed = false
            window.contentView = contentView
            window.center()
            result = (contentView, window, nil, nil)

        case .row:
            let stack = NSStackView()
            stack.orientation = .horizontal
            stack.alignment = .centerY
            stack.distribution = .fill
            stack.spacing = 8
            result = (stack, nil, nil, nil)

        case .column:
            let stack = NSStackView()
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.distribution = .fill
            stack.spacing = 8
            result = (stack, nil, nil, nil)

        case .grid:
            let grid = NSGridView(views: [[NSView]]())
            grid.rowSpacing = 8
            grid.columnSpacing = 8
            result = (grid, nil, nil, nil)

        case .spacer:
            let spacer = NSView(frame: .zero)
            spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
            spacer.setContentHuggingPriority(.defaultLow, for: .vertical)
            result = (spacer, nil, nil, nil)

        case .separator:
            let separator = NSBox(frame: .zero)
            separator.boxType = .separator
            result = (separator, nil, nil, nil)

        case .text:
            let label = NSTextField(labelWithString: "")
            label.maximumNumberOfLines = 0
            label.lineBreakMode = .byWordWrapping
            result = (label, nil, nil, nil)

        case .richText:
            let textView = NSTextView(frame: .zero)
            textView.isEditable = false
            textView.isSelectable = true
            textView.drawsBackground = false
            textView.textContainerInset = NSSize(width: 0, height: 4)
            textView.heightAnchor.constraint(greaterThanOrEqualToConstant: 52).isActive = true
            result = (textView, nil, nil, nil)

        case .button:
            let button = NSButton(title: "Button", target: nil, action: nil)
            button.bezelStyle = .rounded
            let trampoline = ActionTrampoline(nodeID: node.id, eventType: .EVENT_ACTIVATE) { [weak self] nodeID, type in
                self?.onAction?(nodeID, type)
            }
            button.target = trampoline
            button.action = #selector(ActionTrampoline.performAction(_:))
            result = (button, nil, nil, trampoline)

        case .toggle:
            let toggle = NSButton(checkboxWithTitle: "Toggle", target: nil, action: nil)
            result = (toggle, nil, nil, nil)

        case .textInput:
            let field = NSTextField(frame: .zero)
            field.widthAnchor.constraint(greaterThanOrEqualToConstant: 180).isActive = true
            result = (field, nil, nil, nil)

        case .textArea:
            let scrollView = NSScrollView(frame: .zero)
            scrollView.hasVerticalScroller = true
            scrollView.borderType = .bezelBorder
            let textView = NSTextView(frame: .zero)
            textView.isRichText = false
            textView.isEditable = true
            textView.isSelectable = true
            textView.textContainerInset = NSSize(width: 6, height: 6)
            scrollView.documentView = textView
            scrollView.widthAnchor.constraint(greaterThanOrEqualToConstant: 280).isActive = true
            scrollView.heightAnchor.constraint(greaterThanOrEqualToConstant: 88).isActive = true
            result = (scrollView, nil, nil, nil)

        case .progress:
            let progress = NSProgressIndicator(frame: .zero)
            progress.style = .bar
            progress.minValue = 0
            progress.maxValue = 1
            progress.isIndeterminate = false
            progress.widthAnchor.constraint(greaterThanOrEqualToConstant: 150).isActive = true
            result = (progress, nil, nil, nil)

        case .image:
            let imageView = NSImageView(frame: .zero)
            imageView.imageScaling = .scaleProportionallyUpOrDown
            imageView.image = NSImage(systemSymbolName: "photo", accessibilityDescription: "Image")
            imageView.widthAnchor.constraint(greaterThanOrEqualToConstant: 48).isActive = true
            imageView.heightAnchor.constraint(greaterThanOrEqualToConstant: 48).isActive = true
            result = (imageView, nil, nil, nil)

        case .scroll:
            let scrollView = NSScrollView(frame: .zero)
            scrollView.hasVerticalScroller = true
            scrollView.hasHorizontalScroller = false
            scrollView.drawsBackground = false
            // An NSScrollView holds exactly one documentView, so children are attached to an
            // implicit stack; otherwise every child but the last would be evicted silently.
            let documentStack = NSStackView()
            documentStack.orientation = .vertical
            documentStack.alignment = .leading
            documentStack.distribution = .fill
            documentStack.spacing = 8
            documentStack.translatesAutoresizingMaskIntoConstraints = false
            scrollView.documentView = documentStack
            NSLayoutConstraint.activate([
                documentStack.leadingAnchor.constraint(equalTo: scrollView.contentView.leadingAnchor),
                documentStack.trailingAnchor.constraint(equalTo: scrollView.contentView.trailingAnchor),
                documentStack.topAnchor.constraint(equalTo: scrollView.contentView.topAnchor),
                documentStack.widthAnchor.constraint(equalTo: scrollView.contentView.widthAnchor),
            ])
            result = (scrollView, nil, nil, nil)

        case .list, .table:
            let table = makeTable(for: node)
            result = (table.0, table.1, table.2, nil)

        case .tree:
            let outline = makeOutline(for: node)
            result = (outline.0, outline.1, outline.2, nil)

        default:
            throw ControlFactoryError.unsupportedNodeType(node.nodeType)
        }

        if node.nodeType != .surface {
            result.view.translatesAutoresizingMaskIntoConstraints = false
        }
        let handle = RenderHandle(
            nodeID: node.id,
            nodeType: node.nodeType,
            view: result.view,
            window: result.window,
            parentID: node.parentID,
            childIDs: node.orderedChildren,
            modelAdapter: result.adapter,
            actionTrampoline: result.trampoline
        )
        apply(node: node, to: handle)
        return handle
    }

    /// Property entries of `node` in the order they are applied to a handle.
    ///
    /// `Node.properties` is a dictionary, so its iteration order varies between processes.
    /// Properties that write the same AppKit state (`text`/`value`, `selected`/`value`) would
    /// otherwise resolve nondeterministically; ascending `PropertyRef` order fixes the outcome.
    public static func orderedPropertyEntries(of node: Node) -> [(PropertyRef, Value)] {
        node.propertyEntries.sorted { $0.0 < $1.0 }
    }

    public func apply(node: Node, to handle: RenderHandle) {
        for (property, value) in Self.orderedPropertyEntries(of: node) {
            apply(property: property, value: value, to: handle)
        }
    }

    public func apply(property: PropertyRef, value: Value?, to handle: RenderHandle) {
        switch property {
        case .label:
            let label = value?.asString
            handle.accessibilityMetadata.label = label
            handle.view.setAccessibilityLabel(label)
            handle.window?.title = label ?? "SRUI"
            if let button = handle.view as? NSButton {
                button.title = label ?? defaultLabel(for: handle.nodeType)
            }
            if let table = tableView(in: handle) {
                table.tableColumns.first?.title = label ?? defaultLabel(for: handle.nodeType)
            }

        case .accessibleDescription:
            let description = value?.asString
            handle.accessibilityMetadata.description = description
            handle.view.setAccessibilityHelp(description)

        case .valueDescription:
            let description = value?.asString
            handle.accessibilityMetadata.valueDescription = description
            handle.view.setAccessibilityValueDescription(description)

        case .actions:
            handle.accessibilityMetadata.actions = value?.asList?.compactMap { $0.asString } ?? []

        case .visibility:
            // §7.4: `hidden` is invisible but retains layout space; only `collapsed` is
            // removed from layout calculation.
            let token = value?.asEnumToken
            handle.view.isHidden = token == .visibilityCollapsed
            handle.view.alphaValue = token == .visibilityHidden ? 0 : 1

        case .enabled:
            (handle.view as? NSControl)?.isEnabled = value?.asBool ?? true

        case .readOnly:
            let readOnly = value?.asBool ?? false
            if let field = handle.view as? NSTextField {
                field.isEditable = readOnly == false
            }
            textView(in: handle)?.isEditable = readOnly == false

        case .busy:
            guard let progress = handle.view as? NSProgressIndicator else { break }
            let busy = value?.asBool ?? false
            progress.isIndeterminate = busy
            busy ? progress.startAnimation(nil) : progress.stopAnimation(nil)

        case .selected:
            if let button = handle.view as? NSButton {
                button.state = (value?.asBool ?? false) ? .on : .off
            }

        case .text:
            let text = value?.asString ?? ""
            if let field = handle.view as? NSTextField {
                field.stringValue = text
            } else if let textView = textView(in: handle) {
                textView.string = text
            }

        case .value:
            applyValue(value, to: handle)

        case .placeholder:
            (handle.view as? NSTextField)?.placeholderString = value?.asString

        case .resource:
            // lc-debt: no client resource cache exists yet (§14; the Resources target is a stub),
            // so an arriving hash is recorded and the placeholder retained rather than resolved;
            // resolve `pendingResourceHash` through the cache once it lands.
            handle.pendingResourceHash = value?.asResourceHash
            if let imageView = handle.view as? NSImageView {
                imageView.image = NSImage(systemSymbolName: "photo", accessibilityDescription: "Image")
            }
            RendererDiagnostics.log(
                "resource node=\(handle.nodeID) hash=\(handle.pendingResourceHash?.description ?? "none") unresolved (§14)"
            )

        case .items:
            updateCollection(value, in: handle)

        case .horizontalAlignment:
            handle.layoutMetadata.horizontalAlignment = value?.asEnumToken
            applyAlignment(to: handle)

        case .verticalAlignment:
            handle.layoutMetadata.verticalAlignment = value?.asEnumToken
            applyAlignment(to: handle)

        case .grow:
            let priority: NSLayoutConstraint.Priority = (value?.asBool ?? false) ? .defaultLow : .defaultHigh
            handle.view.setContentHuggingPriority(priority, for: .horizontal)
            handle.view.setContentHuggingPriority(priority, for: .vertical)

        case .shrink:
            let priority: NSLayoutConstraint.Priority = (value?.asBool ?? false) ? .defaultLow : .defaultHigh
            handle.view.setContentCompressionResistancePriority(priority, for: .horizontal)
            handle.view.setContentCompressionResistancePriority(priority, for: .vertical)

        case .minimumSize:
            handle.layoutMetadata.minimumSize = value?.asSize
            replaceSizeConstraints(for: property, size: value?.asSize, relation: .minimum, handle: handle)

        case .maximumSize:
            handle.layoutMetadata.maximumSize = value?.asSize
            replaceSizeConstraints(for: property, size: value?.asSize, relation: .maximum, handle: handle)

        case .preferredSize:
            handle.layoutMetadata.preferredSize = value?.asSize
            replaceSizeConstraints(for: property, size: value?.asSize, relation: .preferred, handle: handle)

        case .spacingRole:
            let spacing = spacing(for: value?.asEnumToken)
            if let stack = handle.view as? NSStackView {
                stack.spacing = spacing
            } else if let grid = handle.view as? NSGridView {
                grid.rowSpacing = spacing
                grid.columnSpacing = spacing
            }

        case .paddingRole:
            if let stack = handle.view as? NSStackView {
                let padding = spacing(for: value?.asEnumToken)
                stack.edgeInsets = NSEdgeInsets(
                    top: padding,
                    left: padding,
                    bottom: padding,
                    right: padding
                )
            }

        case .role:
            applyRole(value?.asEnumToken, to: handle)

        case .presentationHint, .validationState, .modelRef, .actionKey, .columns, .selectionMode:
            break

        default:
            break
        }
    }

    private func makeTable(for node: Node) -> (NSView, NSWindow?, AnyObject?) {
        let scrollView = NSScrollView(frame: .zero)
        scrollView.hasVerticalScroller = true
        scrollView.borderType = .bezelBorder

        let tableView = NSTableView(frame: .zero)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("content"))
        column.title = node.nodeType == .list ? "List" : "Table"
        column.width = 360
        tableView.addTableColumn(column)
        tableView.headerView = node.nodeType == .list ? nil : NSTableHeaderView()
        tableView.usesAlternatingRowBackgroundColors = true

        let rows = rows(from: node.getProperty(.items))
        let adapter = TableCollectionAdapter(rows: rows)
        tableView.dataSource = adapter
        tableView.delegate = adapter
        scrollView.documentView = tableView
        scrollView.widthAnchor.constraint(greaterThanOrEqualToConstant: 320).isActive = true
        scrollView.heightAnchor.constraint(greaterThanOrEqualToConstant: 100).isActive = true
        return (scrollView, nil, adapter)
    }

    private func makeOutline(for node: Node) -> (NSView, NSWindow?, AnyObject?) {
        let scrollView = NSScrollView(frame: .zero)
        scrollView.hasVerticalScroller = true
        scrollView.borderType = .bezelBorder

        let outlineView = NSOutlineView(frame: .zero)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("tree"))
        column.title = "Tree"
        column.width = 360
        outlineView.addTableColumn(column)
        outlineView.outlineTableColumn = column
        outlineView.headerView = nil

        let rows = rows(from: node.getProperty(.items))
        let adapter = OutlineCollectionAdapter(rows: rows)
        outlineView.dataSource = adapter
        outlineView.delegate = adapter
        scrollView.documentView = outlineView
        scrollView.widthAnchor.constraint(greaterThanOrEqualToConstant: 320).isActive = true
        scrollView.heightAnchor.constraint(greaterThanOrEqualToConstant: 100).isActive = true
        return (scrollView, nil, adapter)
    }

    private func rows(from value: Value?) -> [String] {
        guard let values = value?.asList else { return [] }
        return values.map { $0.asString ?? $0.description }
    }

    private func updateCollection(_ value: Value?, in handle: RenderHandle) {
        let rows = rows(from: value)
        if let adapter = handle.modelAdapter as? TableCollectionAdapter,
           let tableView = tableView(in: handle) {
            adapter.update(rows: rows, tableView: tableView)
        } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
                  let outlineView = outlineView(in: handle) {
            adapter.update(rows: rows, outlineView: outlineView)
        }
    }

    private func tableView(in handle: RenderHandle) -> NSTableView? {
        (handle.view as? NSScrollView)?.documentView as? NSTableView
    }

    private func outlineView(in handle: RenderHandle) -> NSOutlineView? {
        (handle.view as? NSScrollView)?.documentView as? NSOutlineView
    }

    private func textView(in handle: RenderHandle) -> NSTextView? {
        if let textView = handle.view as? NSTextView {
            return textView
        }
        return (handle.view as? NSScrollView)?.documentView as? NSTextView
    }

    private func applyValue(_ value: Value?, to handle: RenderHandle) {
        if let progress = handle.view as? NSProgressIndicator {
            progress.doubleValue = numericValue(value) ?? 0
        } else if let button = handle.view as? NSButton, handle.nodeType == .toggle {
            button.state = (value?.asBool ?? false) ? .on : .off
        } else if let field = handle.view as? NSTextField {
            // A cleared property blanks the control, but a `value` of some other type is not a
            // string this control can show: ignore it rather than wiping the rendered text on a
            // type mismatch (§4 inv. 13).
            if let value {
                if let string = value.asString { field.stringValue = string }
            } else {
                field.stringValue = ""
            }
        } else if let textView = textView(in: handle) {
            if let value {
                if let string = value.asString { textView.string = string }
            } else {
                textView.string = ""
            }
        }
    }

    private func numericValue(_ value: Value?) -> Double? {
        if let float = value?.asFloat64 { return float }
        if let signed = value?.asSignedInt { return Double(signed) }
        if let unsigned = value?.asUnsignedInt { return Double(unsigned) }
        return nil
    }

    private func defaultLabel(for type: TypeRef) -> String {
        switch type {
        case .button: "Button"
        case .toggle: "Toggle"
        case .list: "List"
        case .table: "Table"
        default: ""
        }
    }

    private func applyRole(_ token: EnumToken?, to handle: RenderHandle) {
        let textRole = token.flatMap(StandardTextRole.init(enumToken:))
        let actionRole = token.flatMap(StandardActionRole.init(enumToken:))

        // A cleared role must restore defaults, otherwise the previous role's styling sticks.
        if textRole != nil || token == nil {
            let style: (font: NSFont, color: NSColor) = switch textRole ?? .body {
            case .title:
                (NSFont.systemFont(ofSize: 24, weight: .bold), .labelColor)
            case .heading:
                (NSFont.systemFont(ofSize: 18, weight: .semibold), .labelColor)
            case .body:
                (NSFont.systemFont(ofSize: 13), .labelColor)
            case .caption:
                (NSFont.systemFont(ofSize: 11), .secondaryLabelColor)
            case .code:
                (NSFont.monospacedSystemFont(ofSize: 13, weight: .regular), .labelColor)
            case .status:
                (NSFont.systemFont(ofSize: 13, weight: .medium), .secondaryLabelColor)
            case .warning:
                (NSFont.systemFont(ofSize: 13, weight: .semibold), .systemOrange)
            case .error:
                (NSFont.systemFont(ofSize: 13, weight: .semibold), .systemRed)
            }
            (handle.view as? NSTextField)?.font = style.font
            (handle.view as? NSTextField)?.textColor = style.color
            textView(in: handle)?.font = style.font
            textView(in: handle)?.textColor = style.color
        }

        guard actionRole != nil || token == nil,
              let button = handle.view as? NSButton else {
            return
        }
        switch actionRole ?? .normal {
        case .normal:
            button.bezelStyle = .rounded
            button.contentTintColor = nil
            button.keyEquivalent = ""
        case .primary:
            button.bezelStyle = .rounded
            button.contentTintColor = .controlAccentColor
            button.keyEquivalent = "\r"
        case .destructive:
            button.bezelStyle = .rounded
            button.contentTintColor = .systemRed
            button.keyEquivalent = ""
        case .quiet:
            button.bezelStyle = .inline
            button.contentTintColor = nil
            button.keyEquivalent = ""
        }
    }

    private func applyAlignment(to handle: RenderHandle) {
        guard let stack = handle.view as? NSStackView else { return }

        // A cleared alignment restores the platform default; a *known* token maps to its AppKit
        // equivalent; an unrecognized or wrong-family token is ignored rather than coerced into an
        // alignment the server never asked for (§4 inv. 13).
        if stack.orientation == .vertical {
            switch handle.layoutMetadata.horizontalAlignment {
            case nil: stack.alignment = .centerX
            case .horizontalAlignmentLeading: stack.alignment = .leading
            case .horizontalAlignmentCenter: stack.alignment = .centerX
            case .horizontalAlignmentTrailing: stack.alignment = .trailing
            case .horizontalAlignmentFill: stack.alignment = .width
            default: break
            }
        } else if stack.orientation == .horizontal {
            switch handle.layoutMetadata.verticalAlignment {
            case nil: stack.alignment = .centerY
            case .verticalAlignmentTop: stack.alignment = .top
            case .verticalAlignmentCenter: stack.alignment = .centerY
            case .verticalAlignmentBottom: stack.alignment = .bottom
            case .verticalAlignmentFill: stack.alignment = .height
            default: break
            }
        }
    }

    private enum SizeRelation {
        case minimum
        case maximum
        case preferred
    }

    private func replaceSizeConstraints(
        for property: PropertyRef,
        size: Size?,
        relation: SizeRelation,
        handle: RenderHandle
    ) {
        handle.propertyConstraints[property]?.forEach { $0.isActive = false }
        guard let size else {
            handle.propertyConstraints[property] = []
            return
        }

        let constraints: [NSLayoutConstraint]
        switch relation {
        case .minimum:
            constraints = [
                handle.view.widthAnchor.constraint(greaterThanOrEqualToConstant: max(0, size.width)),
                handle.view.heightAnchor.constraint(greaterThanOrEqualToConstant: max(0, size.height)),
            ]
        case .maximum:
            constraints = [
                handle.view.widthAnchor.constraint(lessThanOrEqualToConstant: max(0, size.width)),
                handle.view.heightAnchor.constraint(lessThanOrEqualToConstant: max(0, size.height)),
            ]
        case .preferred:
            constraints = [
                handle.view.widthAnchor.constraint(equalToConstant: max(0, size.width)),
                handle.view.heightAnchor.constraint(equalToConstant: max(0, size.height)),
            ]
            constraints.forEach { $0.priority = .defaultHigh }
        }
        NSLayoutConstraint.activate(constraints)
        handle.propertyConstraints[property] = constraints
    }

    private func spacing(for token: EnumToken?) -> CGFloat {
        switch token {
        case .spacingRoleNone, .paddingRoleNone: 0
        case .spacingRoleTight, .paddingRoleTight: 4
        case .spacingRoleRelaxed, .paddingRoleRelaxed: 16
        default: 8
        }
    }
}
