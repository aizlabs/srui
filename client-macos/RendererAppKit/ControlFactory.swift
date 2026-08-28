import AppKit
import SemanticModel

public enum ControlFactoryError: Error, Equatable, Sendable {
    case unsupportedNodeType(TypeRef)
}

/// Creates native controls for the required §7.3 tier and applies scalar properties in place.
@MainActor
public final class ControlFactory {
    public init() {}

    public func makeHandle(for node: Node) throws -> RenderHandle {
        let result: (view: NSView, window: NSWindow?, adapter: AnyObject?)

        switch node.nodeType {
        case .surface:
            let contentView = NSView(frame: .zero)
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 820, height: 720),
                styleMask: [.titled, .closable, .miniaturizable, .resizable],
                backing: .buffered,
                defer: false
            )
            window.contentView = contentView
            window.center()
            result = (contentView, window, nil)

        case .row:
            let stack = NSStackView()
            stack.orientation = .horizontal
            stack.alignment = .centerY
            stack.distribution = .fill
            stack.spacing = 8
            result = (stack, nil, nil)

        case .column:
            let stack = NSStackView()
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.distribution = .fill
            stack.spacing = 8
            result = (stack, nil, nil)

        case .grid:
            let grid = NSGridView(views: [[NSView]]())
            grid.rowSpacing = 8
            grid.columnSpacing = 8
            result = (grid, nil, nil)

        case .spacer:
            let spacer = NSView(frame: .zero)
            spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
            spacer.setContentHuggingPriority(.defaultLow, for: .vertical)
            result = (spacer, nil, nil)

        case .separator:
            let separator = NSBox(frame: .zero)
            separator.boxType = .separator
            result = (separator, nil, nil)

        case .text:
            let label = NSTextField(labelWithString: "")
            label.maximumNumberOfLines = 0
            label.lineBreakMode = .byWordWrapping
            result = (label, nil, nil)

        case .richText:
            let textView = NSTextView(frame: .zero)
            textView.isEditable = false
            textView.isSelectable = true
            textView.drawsBackground = false
            textView.textContainerInset = NSSize(width: 0, height: 4)
            textView.heightAnchor.constraint(greaterThanOrEqualToConstant: 52).isActive = true
            result = (textView, nil, nil)

        case .button:
            let button = NSButton(title: "Button", target: nil, action: nil)
            button.bezelStyle = .rounded
            result = (button, nil, nil)

        case .toggle:
            let toggle = NSButton(checkboxWithTitle: "Toggle", target: nil, action: nil)
            result = (toggle, nil, nil)

        case .textInput:
            let field = NSTextField(frame: .zero)
            field.widthAnchor.constraint(greaterThanOrEqualToConstant: 180).isActive = true
            result = (field, nil, nil)

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
            result = (scrollView, nil, nil)

        case .progress:
            let progress = NSProgressIndicator(frame: .zero)
            progress.style = .bar
            progress.minValue = 0
            progress.maxValue = 1
            progress.isIndeterminate = false
            progress.widthAnchor.constraint(greaterThanOrEqualToConstant: 150).isActive = true
            result = (progress, nil, nil)

        case .image:
            let imageView = NSImageView(frame: .zero)
            imageView.imageScaling = .scaleProportionallyUpOrDown
            imageView.image = NSImage(systemSymbolName: "photo", accessibilityDescription: "Image")
            imageView.widthAnchor.constraint(greaterThanOrEqualToConstant: 48).isActive = true
            imageView.heightAnchor.constraint(greaterThanOrEqualToConstant: 48).isActive = true
            result = (imageView, nil, nil)

        case .scroll:
            let scrollView = NSScrollView(frame: .zero)
            scrollView.hasVerticalScroller = true
            scrollView.hasHorizontalScroller = false
            scrollView.drawsBackground = false
            result = (scrollView, nil, nil)

        case .list, .table:
            result = makeTable(for: node)

        case .tree:
            result = makeOutline(for: node)

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
            modelAdapter: result.adapter
        )
        apply(node: node, to: handle)
        return handle
    }

    public func apply(node: Node, to handle: RenderHandle) {
        for (property, value) in node.propertyEntries {
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
            let token = value?.asEnumToken
            handle.view.isHidden = token == .visibilityHidden || token == .visibilityCollapsed

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
            if let imageView = handle.view as? NSImageView, value == nil {
                imageView.image = NSImage(systemSymbolName: "photo", accessibilityDescription: "Image")
            }

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
        } else if let field = handle.view as? NSTextField, let string = value?.asString {
            field.stringValue = string
        } else if let textView = textView(in: handle), let string = value?.asString {
            textView.string = string
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
        if let textRole = token.flatMap(StandardTextRole.init(enumToken:)) {
            let style: (font: NSFont, color: NSColor) = switch textRole {
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

        guard let actionRole = token.flatMap(StandardActionRole.init(enumToken:)),
              let button = handle.view as? NSButton else {
            return
        }
        switch actionRole {
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

        if stack.orientation == .vertical,
           let token = handle.layoutMetadata.horizontalAlignment {
            switch token {
            case .horizontalAlignmentLeading: stack.alignment = .leading
            case .horizontalAlignmentCenter: stack.alignment = .centerX
            case .horizontalAlignmentTrailing: stack.alignment = .trailing
            case .horizontalAlignmentFill: stack.alignment = .width
            default: break
            }
        } else if stack.orientation == .horizontal,
                  let token = handle.layoutMetadata.verticalAlignment {
            switch token {
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
