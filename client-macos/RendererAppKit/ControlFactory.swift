import AppKit
import SemanticModel
import Collections
import Text
import Terminal

public enum ControlFactoryError: Error, Equatable, Sendable {
    case unsupportedNodeType(TypeRef)
}

/// Target-action trampoline for interactive AppKit controls (§7.6, §7.7, §22).
@MainActor
public final class ActionTrampoline: NSObject {
    public let nodeID: NodeId
    public let handler: @MainActor (SemanticInteraction) -> Void

    public init(
        nodeID: NodeId,
        handler: @escaping @MainActor (SemanticInteraction) -> Void
    ) {
        self.nodeID = nodeID
        self.handler = handler
    }

    @objc public func performButtonAction(_ sender: Any?) {
        handler(.activate(nodeID: nodeID))
    }

    @objc public func performToggleAction(_ sender: Any?) {
        let isOn = (sender as? NSButton)?.state == .on
        handler(.valueChanged(nodeID: nodeID, value: .bool(isOn)))
    }

    @objc public func performAction(_ sender: Any?) {
        performButtonAction(sender)
    }
}

/// Creates native controls for the required §7.3 tier and applies scalar properties in place.
@MainActor
public final class ControlFactory {
    /// Semantic interaction callback invoked when a native interactive control is activated or changed (§7.6, §7.7).
    public var onInteraction: (@MainActor (SemanticInteraction) -> Void)?

    /// Primitive cache-miss callback. Session encodes this as `ClientModelRangeRequest` (§8, §22.7).
    public var onCollectionRangeRequest: (@MainActor (CollectionRangeRequest) -> Void)?

    /// Synchronous main-actor resolver from content hash to a retained `NSImage` (§14).
    ///
    /// Supplied by `AppKitRenderer` so a property apply that references an already-committed
    /// resource paints immediately; a missing hash keeps the system placeholder.
    public var resolveResourceImage: (@MainActor (ResourceHash) -> NSImage?)?

    public let textEditingSession: TextEditingSession
    public let terminalSession: TerminalSession
    public var onTerminalInput: (@MainActor (NodeId, Data) -> Void)?
    public var onTerminalResize: (@MainActor (NodeId, UInt32, UInt32, UInt32, UInt32) -> Void)?

    public let extensionMountResolver: ExtensionMountResolver

    public init(
        textEditingSession: TextEditingSession = TextEditingSession(),
        terminalSession: TerminalSession = TerminalSession(),
        extensionMountResolver: ExtensionMountResolver = ExtensionMountResolver()
    ) {
        self.textEditingSession = textEditingSession
        self.terminalSession = terminalSession
        self.extensionMountResolver = extensionMountResolver
        self.textEditingSession.onCommit = { [weak self] nodeID, text, seq, epoch in
            self?.onInteraction?(.textEdit(nodeID: nodeID, text: text, editSeq: seq, laneEpoch: epoch))
        }
    }

    public func registerExtension(typeRef: TypeRef, kind: ExtensionControlKind) throws {
        try extensionMountResolver.register(typeRef: typeRef, kind: kind)
    }

    public func resetExtensionRegistry() {
        extensionMountResolver.reset()
    }

    public func extensionKind(for typeRef: TypeRef) -> ExtensionControlKind? {
        extensionMountResolver.extensionKind(for: typeRef)
    }

    public func makeHandle(for node: Node, store: SemanticStore? = nil) throws -> RenderHandle {
        try makeHandle(
            for: node,
            store: store,
            mountDecision: extensionMountResolver.decision(for: node, in: store)
        )
    }

    func makeHandle(
        for node: Node,
        store: SemanticStore?,
        mountDecision: ExtensionMountDecision
    ) throws -> RenderHandle {
        let result: (view: NSView, window: NSWindow?, adapter: AnyObject?, trampoline: AnyObject?)
        var textAdapter: NativeTextEditorAdapter?

        switch mountDecision {
        case .native(let kind):
            switch kind {
            case .terminal:
                let view = makeTerminalView(for: node)
                result = (view, nil, nil, nil)
            }
        case .fallback:
            let stack = NSStackView()
            stack.orientation = .vertical
            stack.alignment = .leading
            stack.distribution = .fill
            stack.spacing = 8
            result = (stack, nil, nil, nil)
        case .rejected(let error):
            throw error
        case .standard:
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
            let trampoline = ActionTrampoline(nodeID: node.id) { [weak self] interaction in
                self?.onInteraction?(interaction)
            }
            button.target = trampoline
            button.action = #selector(ActionTrampoline.performButtonAction(_:))
            result = (button, nil, nil, trampoline)

        case .toggle:
            let toggle = NSButton(checkboxWithTitle: "Toggle", target: nil, action: nil)
            let trampoline = ActionTrampoline(nodeID: node.id) { [weak self] interaction in
                self?.onInteraction?(interaction)
            }
            toggle.target = trampoline
            toggle.action = #selector(ActionTrampoline.performToggleAction(_:))
            result = (toggle, nil, nil, trampoline)

        case .textInput:
            let field = NSTextField(frame: .zero)
            field.widthAnchor.constraint(greaterThanOrEqualToConstant: 180).isActive = true
            let adapter = NativeTextEditorAdapter(
                nodeID: node.id,
                session: textEditingSession,
                textField: field
            )
            textAdapter = adapter
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
            let adapter = NativeTextEditorAdapter(
                nodeID: node.id,
                session: textEditingSession,
                textView: textView
            )
            textAdapter = adapter
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
            imageView.image = Self.placeholderImage
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
            let table = makeTable(for: node, store: store)
            result = (table.0, table.1, table.2, nil)

        case .tree:
            let outline = makeOutline(for: node, store: store)
            result = (outline.0, outline.1, outline.2, nil)

        default:
            throw ControlFactoryError.unsupportedNodeType(node.nodeType)
        }
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
            actionTrampoline: result.trampoline,
            textAdapter: textAdapter
        )
        apply(node: node, to: handle, store: store)
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

    /// Property entries of `node` in the order they are applied to a handle.
    public static func shouldSkipTextFallback(for handle: RenderHandle, node: Node) -> Bool {
        handle.textAdapter != nil && node.getProperty(.value) != nil
    }

    /// Displayed editor string: canonical `.value` when present, otherwise `.text`.
    ///
    /// Incremental clears of `.value` must reapply this so they match a remount, which
    /// stops skipping `.text` once `.value` is gone.
    public static func displayedEditorText(for node: Node) -> Value? {
        node.getProperty(.value) ?? node.getProperty(.text)
    }

    public func apply(node: Node, to handle: RenderHandle, store: SemanticStore? = nil) {
        let entries = Self.orderedPropertyEntries(of: node)
        for (property, value) in entries {
            if property == .text, Self.shouldSkipTextFallback(for: handle, node: node) {
                continue
            }
            apply(property: property, value: value, to: handle, store: store)
        }
        if let adapter = handle.textAdapter, Self.displayedEditorText(for: node) == nil {
            // Neither `.value` nor `.text` is defined. Seed the empty store baseline so a
            // later rejection-only ack can revert and a remount can keepLocal (§22.6).
            adapter.applyAuthoritativeString("")
        }
        if handle.nodeType == .table || handle.nodeType == .list || handle.nodeType == .tree {
            refreshCollection(in: handle, for: node, store: store ?? SemanticStore())
        }
    }

    public func apply(property: PropertyRef, value: Value?, to handle: RenderHandle, store: SemanticStore? = nil) {
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
                let adapter = handle.modelAdapter as? TableCollectionAdapter
                let hasExplicitColumns = adapter?.hasExplicitColumns ?? false
                if !hasExplicitColumns {
                    table.tableColumns.first?.title = label ?? defaultLabel(for: handle.nodeType)
                }
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
            switch value?.asEnumToken {
            case .visibilityHidden:
                handle.view.isHidden = false
                handle.view.alphaValue = 0
            case .visibilityCollapsed:
                handle.view.isHidden = true
                handle.view.alphaValue = 1
            default:
                handle.view.isHidden = false
                handle.view.alphaValue = 1
            }

        case .enabled:
            let enabled = value?.asBool ?? true
            if let adapter = handle.textAdapter {
                adapter.applyEnabled(enabled)
            }
            if let control = handle.view as? NSControl {
                control.isEnabled = enabled
            }

        case .readOnly:
            let readOnly = value?.asBool ?? false
            if let adapter = handle.textAdapter {
                adapter.applyReadOnly(readOnly)
            } else {
                if let field = handle.view as? NSTextField {
                    field.isEditable = !readOnly
                }
                if let textView = (handle.view as? NSScrollView)?.documentView as? NSTextView {
                    textView.isEditable = !readOnly
                }
            }

        case .busy:
            let busy = value?.asBool ?? false
            if let progress = handle.view as? NSProgressIndicator {
                progress.isIndeterminate = busy
                if busy {
                    progress.startAnimation(nil)
                } else {
                    progress.stopAnimation(nil)
                }
            }

        case .selected:
            let selected = value?.asBool ?? false
            if let button = handle.view as? NSButton {
                button.state = selected ? .on : .off
            }

        case .text:
            if let adapter = handle.textAdapter {
                adapter.applyAuthoritative(value)
            } else {
                if let field = handle.view as? NSTextField {
                    field.stringValue = value?.asString ?? ""
                }
                if let textView = (handle.view as? NSScrollView)?.documentView as? NSTextView {
                    textView.string = value?.asString ?? ""
                }
                if let textView = handle.view as? NSTextView {
                    textView.string = value?.asString ?? ""
                }
            }

        case .value:
            if let adapter = handle.textAdapter {
                adapter.applyAuthoritative(resolvedEditorValue(value, handle: handle, store: store))
            } else {
                applyValue(value, to: handle)
            }

        case .placeholder:
            if let field = handle.view as? NSTextField {
                field.placeholderString = value?.asString
            }

        case .resource:
            applyResourceProperty(value?.asResourceHash, to: handle)

        case .items, .modelRef, .columns, .selectionMode:
            if let adapter = handle.modelAdapter as? TableCollectionAdapter,
               let tableView = tableView(in: handle) {
                if property == .columns {
                    let colStrings = value?.asList?.compactMap { $0.asString }
                    let fallbackTitle = handle.accessibilityMetadata.label ?? defaultLabel(for: handle.nodeType)
                    reconcileColumns(in: tableView, columns: colStrings, fallbackTitle: fallbackTitle)
                    adapter.hasExplicitColumns = (colStrings?.isEmpty == false)
                    tableView.reloadData()
                } else if property == .selectionMode {
                    let mode = value?.asEnumToken.flatMap(StandardSelectionMode.init(enumToken:)) ?? .none
                    adapter.selectionMode = mode
                    applySelectionMode(mode, to: tableView)
                } else if property == .modelRef {
                    let modelID = value?.asUnsignedInt.map { ModelId($0) }
                    let model = modelID.flatMap { store?.getModel($0) }
                    adapter.update(model: model, modelID: modelID, tableView: tableView)
                } else if property == .items {
                    if adapter.isModelBacked { break }
                    let rows = (value?.asList ?? []).map { val in
                        TableCollectionAdapter.TableRow(itemID: nil, cells: cells(from: val))
                    }
                    adapter.update(rows: rows, tableView: tableView)
                }
            } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
                      let outlineView = outlineView(in: handle) {
                if property == .modelRef {
                    let modelID = value?.asUnsignedInt.map { ModelId($0) }
                    let model = modelID.flatMap { store?.getModel($0) }
                    adapter.update(model: model, modelID: modelID, outlineView: outlineView)
                } else if property == .selectionMode {
                    adapter.selectionMode = value?.asEnumToken.flatMap(StandardSelectionMode.init(enumToken:)) ?? .none
                    applySelectionMode(adapter.selectionMode, to: outlineView)
                } else if property == .items {
                    if adapter.isModelBacked { break }
                    let rows = inlineOutlineRows(from: value)
                    adapter.update(rows: rows, outlineView: outlineView)
                }
            }

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

        case .presentationHint, .actionKey:
            break

        case .validationState:
            handle.textAdapter?.applyValidation(value)

        default:
            break
        }
    }

    public static func columnIdentifier(for title: String, occurrence: Int = 1) -> NSUserInterfaceItemIdentifier {
        TableCollectionAdapter.columnIdentifier(for: title, occurrence: occurrence)
    }

    public func reconcileColumns(
        in tableView: NSTableView,
        columns: [String]?,
        fallbackTitle: String
    ) {
        TableCollectionAdapter.reconcileColumns(
            in: tableView,
            columns: columns,
            fallbackTitle: fallbackTitle
        )
    }

    public func applySelectionMode(_ mode: StandardSelectionMode, to tableView: NSTableView) {
        TableCollectionAdapter.applySelectionMode(mode, to: tableView)
    }

    func configureCollectionScrolling(nested: Bool, handle: RenderHandle) {
        guard let scrollView = handle.view as? NSScrollView else { return }
        if let adapter = handle.modelAdapter as? TableCollectionAdapter,
           let tableView = tableView(in: handle) {
            adapter.setNestedInScroll(nested, scrollView: scrollView, tableView: tableView)
        } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
                  let outlineView = outlineView(in: handle) {
            adapter.setNestedInScroll(nested, scrollView: scrollView, outlineView: outlineView)
        }
    }

    public func refreshCollection(
        in handle: RenderHandle,
        for node: Node,
        store: SemanticStore
    ) {
        if let adapter = handle.modelAdapter as? TableCollectionAdapter,
           let tableView = tableView(in: handle) {
            let columnValues = node.getProperty(.columns)?.asList?.compactMap { $0.asString }
            let fallbackTitle = node.getProperty(.label)?.asString ?? defaultLabel(for: handle.nodeType)
            reconcileColumns(in: tableView, columns: columnValues, fallbackTitle: fallbackTitle)
            adapter.hasExplicitColumns = (columnValues?.isEmpty == false)

            let mode = node.getProperty(.selectionMode)?.asEnumToken.flatMap(StandardSelectionMode.init(enumToken:)) ?? .none
            adapter.selectionMode = mode
            applySelectionMode(mode, to: tableView)

            let newRows = tableRows(for: node, store: store)
            if let modelID = node.modelRef {
                adapter.update(model: store.getModel(modelID), modelID: modelID, tableView: tableView)
            } else {
                adapter.update(rows: newRows, tableView: tableView)
            }
        } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
                  let outlineView = outlineView(in: handle) {
            if let modelID = node.modelRef {
                adapter.update(model: store.getModel(modelID), modelID: modelID, outlineView: outlineView)
            } else {
                let rows = inlineOutlineRows(from: node.getProperty(.items))
                adapter.update(rows: rows, outlineView: outlineView)
            }
            let mode = node.getProperty(.selectionMode)?.asEnumToken.flatMap(StandardSelectionMode.init(enumToken:)) ?? .none
            adapter.selectionMode = mode
            applySelectionMode(mode, to: outlineView)
        }
    }

    private func applyResourceProperty(_ hash: ResourceHash?, to handle: RenderHandle) {
        let previous = handle.pendingResourceHash
        handle.pendingResourceHash = hash

        guard let imageView = handle.view as? NSImageView else { return }

        // Clearing or replacing the hash must drop any previously displayed bitmap so a stale
        // image cannot outlive the property that authorized it (§14).
        guard let hash else {
            imageView.image = Self.placeholderImage
            if previous != nil {
                RendererDiagnostics.log("resource node=\(handle.nodeID) cleared (§14)")
            }
            return
        }

        if let resolved = resolveResourceImage?(hash) {
            imageView.image = resolved
            RendererDiagnostics.log("resource node=\(handle.nodeID) hash=\(hash) resolved (§14)")
        } else {
            imageView.image = Self.placeholderImage
            RendererDiagnostics.log(
                "resource node=\(handle.nodeID) hash=\(hash) unresolved (§14)"
            )
        }
    }

    private static let placeholderImage: NSImage =
        NSImage(systemSymbolName: "photo", accessibilityDescription: "Image")
        ?? NSImage(size: NSSize(width: 48, height: 48))

    private func makeTable(for node: Node, store: SemanticStore?) -> (NSView, NSWindow?, AnyObject?) {
        let scrollView = NSScrollView(frame: .zero)
        scrollView.hasVerticalScroller = true
        scrollView.borderType = .bezelBorder

        let tableView = NSTableView(frame: .zero)
        TableCollectionAdapter.applyChrome(isList: node.nodeType == .list, to: tableView)

        let columnValues = node.getProperty(.columns)?.asList?.compactMap { $0.asString }
        let fallbackTitle = node.getProperty(.label)?.asString ?? defaultLabel(for: node.nodeType)
        reconcileColumns(in: tableView, columns: columnValues, fallbackTitle: fallbackTitle)

        let modeToken = node.getProperty(.selectionMode)?.asEnumToken
        let selectionMode = modeToken.flatMap(StandardSelectionMode.init(enumToken:)) ?? .none
        applySelectionMode(selectionMode, to: tableView)

        let modelID = node.modelRef
        let model = modelID.flatMap { store?.getModel($0) }
        let rows = modelID == nil ? tableRows(for: node, store: store) : []
        let adapter = TableCollectionAdapter(
            nodeID: node.id,
            rows: rows,
            model: model,
            modelID: modelID,
            hasExplicitColumns: columnValues?.isEmpty == false,
            selectionMode: selectionMode,
            onSelectionChanged: { [weak self] nodeID, itemID in
                self?.onInteraction?(.selectionChanged(nodeID: nodeID, itemID: itemID))
            },
            onRangeRequest: { [weak self] request in
                self?.onCollectionRangeRequest?(request)
            }
        )
        tableView.dataSource = adapter
        tableView.delegate = adapter
        scrollView.documentView = tableView
        scrollView.widthAnchor.constraint(greaterThanOrEqualToConstant: 320).isActive = true
        let minHeight = scrollView.heightAnchor.constraint(greaterThanOrEqualToConstant: 100)
        minHeight.isActive = true
        adapter.minHeightConstraint = minHeight
        adapter.attachViewportObservation(scrollView: scrollView, tableView: tableView)
        return (scrollView, nil, adapter)
    }

    private func makeOutline(for node: Node, store: SemanticStore?) -> (NSView, NSWindow?, AnyObject?) {
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

        let modelID = node.modelRef
        let model = modelID.flatMap { store?.getModel($0) }
        let modeToken = node.getProperty(.selectionMode)?.asEnumToken
        let selectionMode = modeToken.flatMap(StandardSelectionMode.init(enumToken:)) ?? .none
        TableCollectionAdapter.applySelectionMode(selectionMode, to: outlineView)
        let rows = modelID == nil ? inlineOutlineRows(from: node.getProperty(.items)) : []
        let adapter = OutlineCollectionAdapter(
            nodeID: node.id,
            rows: rows,
            model: model,
            modelID: modelID,
            selectionMode: selectionMode,
            onSelectionChanged: { [weak self] nodeID, itemID in
                self?.onInteraction?(.selectionChanged(nodeID: nodeID, itemID: itemID))
            },
            onRangeRequest: { [weak self] request in
                self?.onCollectionRangeRequest?(request)
            }
        )
        outlineView.dataSource = adapter
        outlineView.delegate = adapter
        scrollView.documentView = outlineView
        scrollView.widthAnchor.constraint(greaterThanOrEqualToConstant: 320).isActive = true
        let minHeight = scrollView.heightAnchor.constraint(greaterThanOrEqualToConstant: 100)
        minHeight.isActive = true
        adapter.minHeightConstraint = minHeight
        adapter.attachViewportObservation(scrollView: scrollView, outlineView: outlineView)
        return (scrollView, nil, adapter)
    }

    private func cells(from value: Value) -> [String] {
        CollectionCells.cells(from: value)
    }

    public func tableRows(for node: Node, store: SemanticStore?) -> [TableCollectionAdapter.TableRow] {
        if let modelID = node.modelRef {
            if let store, let model = store.getModel(modelID) {
                return model.iterCachedItems().map { (_, item) in
                    TableCollectionAdapter.TableRow(
                        itemID: item.itemID,
                        cells: cells(from: item.value)
                    )
                }
            } else {
                return []
            }
        } else {
            guard let values = node.getProperty(.items)?.asList else { return [] }
            return values.map { val in
                TableCollectionAdapter.TableRow(itemID: nil, cells: cells(from: val))
            }
        }
    }

    private func inlineOutlineRows(from value: Value?) -> [String] {
        guard let values = value?.asList else { return [] }
        return values.map { $0.asString ?? ($0 == .null ? "" : $0.description) }
    }

    private func tableView(in handle: RenderHandle) -> NSTableView? {
        guard let table = (handle.view as? NSScrollView)?.documentView as? NSTableView,
              !(table is NSOutlineView) else {
            return nil
        }
        return table
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

    /// When canonical `.value` is cleared, reapply remaining `.text` from the updated node.
    private func resolvedEditorValue(
        _ value: Value?,
        handle: RenderHandle,
        store: SemanticStore?
    ) -> Value? {
        if value != nil {
            return value
        }
        guard let node = store?.getNode(handle.nodeID) else {
            return nil
        }
        return Self.displayedEditorText(for: node)
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

    private func makeTerminalView(for node: Node) -> TerminalView {
        let view = TerminalView(nodeID: node.id)
        view.setContentHuggingPriority(.defaultLow, for: .horizontal)
        view.setContentHuggingPriority(.defaultLow, for: .vertical)
        view.heightAnchor.constraint(greaterThanOrEqualToConstant: 160).isActive = true
        view.onInput = { [weak self] data in
            self?.onTerminalInput?(node.id, data)
        }
        view.onResize = { [weak self] cols, rows, width, height in
            Task { @MainActor in
                await self?.terminalSession.resize(streamID: node.id, columns: Int(cols), rows: Int(rows))
            }
            self?.onTerminalResize?(node.id, cols, rows, width, height)
        }
        let session = terminalSession
        let streamID = node.id
        view.onAcknowledgeRedraw = {
            Task { @MainActor in
                await session.acknowledgeRedraw(streamID: streamID)
            }
        }
        view.snapshotSubscriptionTask = Task { @MainActor [weak view] in
            if let current = await session.snapshot(for: streamID) {
                view?.apply(current)
            }
            for await snapshot in await session.snapshots(for: streamID) {
                guard let view else { break }
                view.apply(snapshot)
            }
        }
        return view
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
