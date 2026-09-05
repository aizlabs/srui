import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
@testable import Collections

private let controlFactoryRequiredTierTypes: [TypeRef] = [
    .surface, .row, .column, .grid, .spacer, .separator, .scroll,
    .text, .richText, .button, .toggle, .textInput, .textArea,
    .progress, .image, .list, .table, .tree,
]

private let controlFactoryUnsupportedTypes: [TypeRef] = [
    .dialog, .select, .choiceGroup, .slider, .numberInput,
    .tabs, .split, .menu, .toolbar,
]

@MainActor
struct ControlFactoryTests {
    @Test(arguments: controlFactoryRequiredTierTypes)
    func requiredTierCreatesExpectedViewAndDefaults(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))

        #expect(handle.nodeType == nodeType)
        #expect(handle.nodeID == 1)

        switch nodeType {
        case .surface:
            let stack = try #require(handle.view as? NSStackView)
            #expect(stack.orientation == .vertical)
            #expect(stack.alignment == .leading)
            #expect(stack.distribution == .fill)
            #expect(stack.spacing == 14)
            #expect(stack.edgeInsets.top == 20)
            #expect(stack.edgeInsets.left == 20)
            #expect(stack.edgeInsets.bottom == 20)
            #expect(stack.edgeInsets.right == 20)
            #expect(handle.window != nil)
            #expect(handle.modelAdapter == nil)
            #expect(handle.actionTrampoline == nil)

        case .row:
            let stack = try #require(handle.view as? NSStackView)
            #expect(stack.orientation == .horizontal)
            #expect(stack.alignment == .centerY)
            #expect(stack.spacing == 8)
            #expect(handle.window == nil)
            #expect(handle.modelAdapter == nil)

        case .column:
            let stack = try #require(handle.view as? NSStackView)
            #expect(stack.orientation == .vertical)
            #expect(stack.alignment == .leading)
            #expect(stack.spacing == 8)
            #expect(handle.modelAdapter == nil)

        case .grid:
            let grid = try #require(handle.view as? NSGridView)
            #expect(grid.rowSpacing == 8)
            #expect(grid.columnSpacing == 8)
            #expect(handle.modelAdapter == nil)

        case .spacer:
            #expect(handle.view is NSStackView == false)
            #expect(
                handle.view.contentHuggingPriority(for: .horizontal).rawValue
                    <= NSLayoutConstraint.Priority.defaultLow.rawValue
            )
            #expect(
                handle.view.contentHuggingPriority(for: .vertical).rawValue
                    <= NSLayoutConstraint.Priority.defaultLow.rawValue
            )

        case .separator:
            let separator = try #require(handle.view as? NSBox)
            #expect(separator.boxType == .separator)

        case .scroll:
            let scroll = try #require(handle.view as? NSScrollView)
            #expect(scroll.hasVerticalScroller)
            #expect(scroll.hasHorizontalScroller == false)
            #expect(scroll.drawsBackground == false)
            let document = try #require(scroll.documentView as? NSStackView)
            #expect(document.orientation == .vertical)
            #expect(document.alignment == .leading)

        case .text:
            let label = try #require(handle.view as? NSTextField)
            #expect(label.isEditable == false)
            #expect(label.isBezeled == false)
            #expect(label.maximumNumberOfLines == 0)
            #expect(label.lineBreakMode == .byWordWrapping)

        case .richText:
            let textView = try #require(handle.view as? NSTextView)
            #expect(textView.isEditable == false)
            #expect(textView.isSelectable)
            #expect(textView.drawsBackground == false)

        case .button:
            let button = try #require(handle.view as? NSButton)
            #expect(button.bezelStyle == .rounded)
            #expect(button.title == "Button")
            #expect(handle.actionTrampoline is ActionTrampoline)

        case .toggle:
            let toggle = try #require(handle.view as? NSButton)
            #expect(toggle.title == "Toggle")
            #expect(toggle.state == .off)
            #expect(handle.actionTrampoline is ActionTrampoline)

        case .textInput:
            let field = try #require(handle.view as? NSTextField)
            #expect(field.isEditable)
            #expect(field.isBezeled)

        case .textArea:
            let scroll = try #require(handle.view as? NSScrollView)
            #expect(scroll.hasVerticalScroller)
            let textView = try #require(scroll.documentView as? NSTextView)
            #expect(textView.isEditable)
            #expect(textView.isRichText == false)

        case .progress:
            let progress = try #require(handle.view as? NSProgressIndicator)
            #expect(progress.style == .bar)
            #expect(progress.minValue == 0)
            #expect(progress.maxValue == 1)
            #expect(progress.isIndeterminate == false)
            #expect(progress.doubleValue == 0)

        case .image:
            let imageView = try #require(handle.view as? NSImageView)
            #expect(imageView.image != nil)
            #expect(imageView.imageScaling == .scaleProportionallyUpOrDown)

        case .list:
            let scroll = try #require(handle.view as? NSScrollView)
            let table = try #require(scroll.documentView as? NSTableView)
            #expect(table.headerView == nil)
            #expect(table.usesAlternatingRowBackgroundColors == false)
            #expect(table.style == .plain)
            #expect(handle.modelAdapter is TableCollectionAdapter)
            let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)
            #expect(adapter.rows.isEmpty)

        case .table:
            let scroll = try #require(handle.view as? NSScrollView)
            let table = try #require(scroll.documentView as? NSTableView)
            #expect(table.headerView != nil)
            #expect(table.usesAlternatingRowBackgroundColors)
            #expect(table.style == .fullWidth)
            #expect(handle.modelAdapter is TableCollectionAdapter)

        case .tree:
            let scroll = try #require(handle.view as? NSScrollView)
            let outline = try #require(scroll.documentView as? NSOutlineView)
            #expect(outline.headerView == nil)
            #expect(handle.modelAdapter is OutlineCollectionAdapter)
            let adapter = try #require(handle.modelAdapter as? OutlineCollectionAdapter)
            #expect(outline.dataSource === adapter)
            #expect(outline.delegate === adapter)

        default:
            Issue.record("Unexpected node type in required-tier matrix: \(nodeType)")
        }

        if nodeType != .surface {
            #expect(handle.view.translatesAutoresizingMaskIntoConstraints == false)
        }
    }

    @Test(arguments: [TypeRef.list, .table, .tree])
    func collectionTypesSeedAndUpdateItems(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(
            for: Node(
                id: 1,
                nodeType: nodeType,
                properties: [.items: .list([.string("Alpha"), .string("Beta")])]
            )
        )

        switch nodeType {
        case .list, .table:
            let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)
            #expect(adapter.rows.map(\.cells) == [["Alpha"], ["Beta"]])
            let table = try #require(
                (handle.view as? NSScrollView)?.documentView as? NSTableView
            )
            #expect(table.numberOfRows == 2)

            factory.apply(
                property: .items,
                value: .list([.string("Gamma")]),
                to: handle
            )
            #expect(adapter.rows.map(\.cells) == [["Gamma"]])
            #expect(table.numberOfRows == 1)

        case .tree:
            _ = try #require(handle.modelAdapter as? OutlineCollectionAdapter)
            let outline = try #require(
                (handle.view as? NSScrollView)?.documentView as? NSOutlineView
            )
            #expect(outline.numberOfRows == 2)

            factory.apply(
                property: .items,
                value: .list([.string("Root")]),
                to: handle
            )
            #expect(outline.numberOfRows == 1)

        default:
            Issue.record("Expected a collection node type, got \(nodeType)")
        }

        factory.apply(property: .items, value: nil, to: handle)
        if let adapter = handle.modelAdapter as? TableCollectionAdapter {
            #expect(adapter.rows.isEmpty)
        }
    }

    @Test
    func buttonAndToggleEmitSemanticInteractions() throws {
        let factory = ControlFactory()
        var receivedInteractions: [SemanticInteraction] = []
        factory.onInteraction = { interaction in
            receivedInteractions.append(interaction)
        }

        let buttonHandle = try factory.makeHandle(for: Node(id: 10, nodeType: .button))
        let button = try #require(buttonHandle.view as? NSButton)
        let buttonTrampoline = try #require(buttonHandle.actionTrampoline as? ActionTrampoline)
        buttonTrampoline.performButtonAction(button)

        #expect(receivedInteractions == [.activate(nodeID: 10)])

        let toggleHandle = try factory.makeHandle(for: Node(id: 11, nodeType: .toggle))
        let toggle = try #require(toggleHandle.view as? NSButton)
        let toggleTrampoline = try #require(toggleHandle.actionTrampoline as? ActionTrampoline)

        toggle.state = .on
        toggleTrampoline.performToggleAction(toggle)
        #expect(receivedInteractions == [.activate(nodeID: 10), .valueChanged(nodeID: 11, value: .bool(true))])

        toggle.state = .off
        toggleTrampoline.performToggleAction(toggle)
        #expect(receivedInteractions == [
            .activate(nodeID: 10),
            .valueChanged(nodeID: 11, value: .bool(true)),
            .valueChanged(nodeID: 11, value: .bool(false)),
        ])
    }

    @Test
    func modelBackedTableResolvesModelWithFourColumnsAndCells() throws {
        var store = SemanticStore()
        let modelID = ModelId(42)
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
        try store.modelInsert(
            id: modelID,
            index: 0,
            items: [
                ModelItem(
                    itemID: ItemId(101),
                    value: .list([.string("Row1-C1"), .string("Row1-C2"), .string("Row1-C3"), .string("Row1-C4")])
                ),
                ModelItem(
                    itemID: ItemId(102),
                    value: .list([.string("Row2-C1"), .string("Row2-C2"), .string("Row2-C3"), .string("Row2-C4")])
                ),
            ]
        )

        let tableNode = Node(
            id: 1,
            nodeType: .table,
            properties: [
                .columns: .list([.string("Col 1"), .string("Col 2"), .string("Col 3"), .string("Col 4")]),
                .modelRef: .unsignedInt(modelID.value),
                .selectionMode: .enumToken(.selectionModeSingle),
            ]
        )
        try store.createNode(id: 1, nodeType: .table, properties: tableNode.propertyEntries)

        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: tableNode, store: store)

        let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)
        let tableView = try #require((handle.view as? NSScrollView)?.documentView as? NSTableView)

        #expect(tableView.tableColumns.count == 4)
        #expect(tableView.tableColumns[0].title == "Col 1")
        #expect(tableView.tableColumns[0].identifier == TableCollectionAdapter.columnIdentifier(for: "Col 1"))
        #expect(tableView.tableColumns[3].title == "Col 4")
        #expect(tableView.tableColumns[3].identifier == TableCollectionAdapter.columnIdentifier(for: "Col 4"))

        #expect(adapter.rows.count == 2)
        #expect(adapter.rows[0].itemID == ItemId(101))
        #expect(adapter.rows[0].cells == ["Row1-C1", "Row1-C2", "Row1-C3", "Row1-C4"])
        #expect(adapter.rows[1].itemID == ItemId(102))
        #expect(adapter.rows[1].cells == ["Row2-C1", "Row2-C2", "Row2-C3", "Row2-C4"])

        // Check cell view rendering
        let cellView0 = try #require(adapter.tableView(tableView, viewFor: tableView.tableColumns[0], row: 0) as? NSTextField)
        #expect(cellView0.stringValue == "Row1-C1")
        let cellView3 = try #require(adapter.tableView(tableView, viewFor: tableView.tableColumns[3], row: 1) as? NSTextField)
        #expect(cellView3.stringValue == "Row2-C4")
    }

    @Test
    func modelRefPresentWithAbsentOrUncachedModelDoesNotFallBackToInlineItems() throws {
        var store = SemanticStore()
        let absentModelID = ModelId(999)
        let uncachedModelID = ModelId(998)
        try store.createModel(id: uncachedModelID, modelType: .table, itemCount: 2)

        let factory = ControlFactory()
        let absentNode = Node(
            id: 1,
            nodeType: .table,
            properties: [
                .modelRef: .unsignedInt(absentModelID.value),
                .items: .list([.string("Fallback Item A"), .string("Fallback Item B")]),
            ]
        )
        let absentHandle = try factory.makeHandle(for: absentNode, store: store)
        let absentAdapter = try #require(absentHandle.modelAdapter as? TableCollectionAdapter)
        let absentTable = try #require((absentHandle.view as? NSScrollView)?.documentView as? NSTableView)
        #expect(absentAdapter.rows.isEmpty)
        #expect(absentAdapter.numberOfRows(in: absentTable) == 0)

        let uncachedNode = Node(
            id: 2,
            nodeType: .table,
            properties: [
                .modelRef: .unsignedInt(uncachedModelID.value),
                .items: .list([.string("Fallback Item A"), .string("Fallback Item B")]),
            ]
        )
        let uncachedHandle = try factory.makeHandle(for: uncachedNode, store: store)
        let uncachedAdapter = try #require(uncachedHandle.modelAdapter as? TableCollectionAdapter)
        let uncachedTable = try #require((uncachedHandle.view as? NSScrollView)?.documentView as? NSTableView)
        TableCollectionAdapter.reconcileColumns(in: uncachedTable, columns: nil, fallbackTitle: "Table")
        #expect(uncachedAdapter.rows.isEmpty)
        #expect(uncachedAdapter.numberOfRows(in: uncachedTable) == 2)
        let loading = try #require(
            uncachedAdapter.tableView(uncachedTable, viewFor: uncachedTable.tableColumns[0], row: 1) as? NSTextField
        )
        #expect(loading.stringValue == CollectionCells.loadingPlaceholder)
        #expect(uncachedAdapter.tableView(uncachedTable, shouldSelectRow: 1) == false)
    }

    @Test
    func selectionModesAndUserEventRouting() throws {
        let factory = ControlFactory()
        var emittedInteractions: [SemanticInteraction] = []
        factory.onInteraction = { emittedInteractions.append($0) }

        let rows = [
            TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Alpha"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["Beta"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(3), cells: ["Gamma"]),
        ]

        // 1. None selection mode rejects selection
        let noneNode = Node(
            id: 1,
            nodeType: .table,
            properties: [
                .selectionMode: .enumToken(.selectionModeNone),
            ]
        )
        let noneHandle = try factory.makeHandle(for: noneNode)
        let noneTable = try #require((noneHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let noneAdapter = try #require(noneHandle.modelAdapter as? TableCollectionAdapter)
        noneAdapter.update(rows: rows, tableView: noneTable)
        #expect(noneTable.selectionHighlightStyle == .none)
        #expect(noneAdapter.tableView(noneTable, shouldSelectRow: 0) == false)

        // 2. Single selection mode emits exactly one event for genuine user selection
        let singleNode = Node(
            id: 2,
            nodeType: .table,
            properties: [
                .selectionMode: .enumToken(.selectionModeSingle),
            ]
        )
        let singleHandle = try factory.makeHandle(for: singleNode)
        let singleTable = try #require((singleHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let singleAdapter = try #require(singleHandle.modelAdapter as? TableCollectionAdapter)
        singleAdapter.update(rows: rows, tableView: singleTable)
        #expect(singleTable.allowsMultipleSelection == false)
        #expect(singleTable.selectionHighlightStyle == .regular)

        singleTable.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        #expect(emittedInteractions == [.selectionChanged(nodeID: 2, itemID: ItemId(2))])
        emittedInteractions.removeAll()

        singleAdapter.selectionMode = .none
        factory.applySelectionMode(.none, to: singleTable)
        #expect(singleTable.selectedRow == -1)
        #expect(emittedInteractions.isEmpty)

        // 3. Multiple selection mode emits one event per selected item ID
        let multipleNode = Node(
            id: 3,
            nodeType: .table,
            properties: [
                .selectionMode: .enumToken(.selectionModeMultiple),
            ]
        )
        let multipleHandle = try factory.makeHandle(for: multipleNode)
        let multipleTable = try #require((multipleHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let multipleAdapter = try #require(multipleHandle.modelAdapter as? TableCollectionAdapter)
        multipleAdapter.update(rows: rows, tableView: multipleTable)
        #expect(multipleTable.allowsMultipleSelection == true)

        multipleTable.selectRowIndexes(IndexSet([0, 2]), byExtendingSelection: false)
        #expect(emittedInteractions == [
            .selectionChanged(nodeID: 3, itemID: ItemId(1)),
            .selectionChanged(nodeID: 3, itemID: ItemId(3)),
        ])
    }

    @Test
    func adapterUpdatePreservesSelectionByItemIdWithZeroRestorationEvents() throws {
        let factory = ControlFactory()
        var emittedInteractions: [SemanticInteraction] = []
        factory.onInteraction = { emittedInteractions.append($0) }

        let node = Node(
            id: 1,
            nodeType: .table,
            properties: [
                .selectionMode: .enumToken(.selectionModeSingle),
            ]
        )
        let handle = try factory.makeHandle(for: node)
        let tableView = try #require((handle.view as? NSScrollView)?.documentView as? NSTableView)
        let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)

        let initialRows = [
            TableCollectionAdapter.TableRow(itemID: ItemId(10), cells: ["Row 10"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(20), cells: ["Row 20"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(30), cells: ["Row 30"]),
        ]
        adapter.update(rows: initialRows, tableView: tableView)

        // Select row 1 (itemID 20)
        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        #expect(tableView.selectedRow == 1)
        emittedInteractions.removeAll()

        // Update rows with insertion at index 0 (shifting itemID 20 to index 2)
        let updatedRows = [
            TableCollectionAdapter.TableRow(itemID: ItemId(5), cells: ["Row 5 (New)"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(10), cells: ["Row 10"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(20), cells: ["Row 20"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(30), cells: ["Row 30"]),
        ]
        adapter.update(rows: updatedRows, tableView: tableView)

        // Selection should be preserved at new index 2
        #expect(tableView.selectedRow == 2)
        // Zero restoration events emitted
        #expect(emittedInteractions.isEmpty)
    }

    @Test(arguments: [
        (TypeRef.row, PropertyRef.spacingRole, Value.enumToken(.spacingRoleTight)),
        (TypeRef.column, PropertyRef.paddingRole, Value.enumToken(.paddingRoleRelaxed)),
        (TypeRef.textInput, PropertyRef.placeholder, Value.string("Enter text")),
        (TypeRef.progress, PropertyRef.value, Value.float64(0.42)),
        (TypeRef.button, PropertyRef.label, Value.string("Save")),
    ] as [(TypeRef, PropertyRef, Value)])
    func propertySetClearRestoresDefaults(
        nodeType: TypeRef,
        property: PropertyRef,
        value: Value
    ) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))

        switch (nodeType, property) {
        case (.row, .spacingRole):
            let stack = try #require(handle.view as? NSStackView)
            #expect(stack.spacing == 8)

        case (.column, .paddingRole):
            let stack = try #require(handle.view as? NSStackView)
            #expect(stack.edgeInsets.top == 0)
            #expect(stack.edgeInsets.left == 0)

        case (.textInput, .placeholder):
            let field = try #require(handle.view as? NSTextField)
            #expect(field.placeholderString == nil)

        case (.progress, .value):
            let progress = try #require(handle.view as? NSProgressIndicator)
            #expect(progress.doubleValue == 0)

        case (.button, .label):
            let button = try #require(handle.view as? NSButton)
            #expect(button.title == "Button")

        default:
            Issue.record("Unexpected symmetry pair \((nodeType, property))")
        }

        factory.apply(property: property, value: value, to: handle)

        switch (nodeType, property) {
        case (.row, .spacingRole):
            #expect((handle.view as? NSStackView)?.spacing == 4)
        case (.column, .paddingRole):
            #expect((handle.view as? NSStackView)?.edgeInsets.top == 16)
        case (.textInput, .placeholder):
            #expect((handle.view as? NSTextField)?.placeholderString == "Enter text")
        case (.progress, .value):
            #expect((handle.view as? NSProgressIndicator)?.doubleValue == 0.42)
        case (.button, .label):
            #expect((handle.view as? NSButton)?.title == "Save")
        default:
            break
        }

        factory.apply(property: property, value: nil, to: handle)

        switch (nodeType, property) {
        case (.row, .spacingRole):
            #expect((handle.view as? NSStackView)?.spacing == 8)
        case (.column, .paddingRole):
            let insets = try #require((handle.view as? NSStackView)?.edgeInsets)
            #expect(insets.top == 8)
            #expect(insets.left == 8)
            #expect(insets.bottom == 8)
            #expect(insets.right == 8)
        case (.textInput, .placeholder):
            #expect((handle.view as? NSTextField)?.placeholderString == nil)
        case (.progress, .value):
            #expect((handle.view as? NSProgressIndicator)?.doubleValue == 0)
        case (.button, .label):
            #expect((handle.view as? NSButton)?.title == "Button")
        default:
            break
        }
    }

    @Test(arguments: controlFactoryUnsupportedTypes)
    func unsupportedNodeTypeThrows(nodeType: TypeRef) {
        #expect(throws: ControlFactoryError.unsupportedNodeType(nodeType)) {
            try ControlFactory().makeHandle(for: Node(id: 1, nodeType: nodeType))
        }
    }

    @Test
    func hiddenRetainsLayoutSpaceWhileCollapsedRemovesIt() throws {
        let factory = ControlFactory()
        let hidden = try factory.makeHandle(for: Node(id: 1, nodeType: .text))
        let collapsed = try factory.makeHandle(for: Node(id: 2, nodeType: .text))

        factory.apply(
            property: .visibility,
            value: .enumToken(.visibilityHidden),
            to: hidden
        )
        factory.apply(
            property: .visibility,
            value: .enumToken(.visibilityCollapsed),
            to: collapsed
        )

        // §7.4 / registry.yaml: `hidden` is invisible but retains layout space;
        // `collapsed` is invisible and removed from layout.
        #expect(hidden.view.isHidden == false)
        #expect(hidden.view.alphaValue == 0)
        #expect(collapsed.view.isHidden)

        factory.apply(
            property: .visibility,
            value: .enumToken(.visibilityVisible),
            to: hidden
        )
        #expect(hidden.view.isHidden == false)
        #expect(hidden.view.alphaValue == 1)
    }

    @Test
    func clearingRoleRestoresDefaultTextStyling() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(
            for: Node(
                id: 1,
                nodeType: .text,
                properties: [.role: .enumToken(.textRoleTitle)]
            )
        )
        let label = try #require(handle.view as? NSTextField)
        let styledFont = try #require(label.font)
        #expect(styledFont.pointSize == 24)

        factory.apply(property: .role, value: nil, to: handle)

        let clearedFont = try #require(label.font)
        #expect(clearedFont.pointSize == 13)
        #expect(label.textColor == .labelColor)
    }

    @Test
    func clearingRoleRestoresDefaultButtonStyling() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(
            for: Node(
                id: 1,
                nodeType: .button,
                properties: [.role: .enumToken(.actionRoleDestructive)]
            )
        )
        let button = try #require(handle.view as? NSButton)
        #expect(button.contentTintColor == .systemRed)

        factory.apply(property: .role, value: nil, to: handle)

        #expect(button.contentTintColor == nil)
        #expect(button.keyEquivalent.isEmpty)
    }

    @Test
    func resourceArrivalIsRecordedOnTheHandle() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .image))
        let hash = try ResourceHash(rawBytes: Array(repeating: 0xab, count: 32))

        factory.apply(property: .resource, value: .resourceHash(hash), to: handle)
        #expect(handle.pendingResourceHash == hash)

        factory.apply(property: .resource, value: nil, to: handle)
        #expect(handle.pendingResourceHash == nil)
    }

    @Test
    func resourcePropertyUsesResolverWhenCachedAndPlaceholderWhenMissing() throws {
        let factory = ControlFactory()
        let hash = try ResourceHash(rawBytes: Array(repeating: 0xcd, count: 32))
        let cachedImage = NSImage(size: NSSize(width: 2, height: 2), flipped: false) { rect in
            NSColor.red.setFill()
            rect.fill()
            return true
        }

        factory.resolveResourceImage = { candidate in
            candidate == hash ? cachedImage : nil
        }

        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .image))
        let imageView = try #require(handle.view as? NSImageView)
        let placeholder = imageView.image

        factory.apply(property: .resource, value: .resourceHash(hash), to: handle)
        #expect(handle.pendingResourceHash == hash)
        #expect(imageView.image === cachedImage)

        let missing = try ResourceHash(rawBytes: Array(repeating: 0xee, count: 32))
        factory.apply(property: .resource, value: .resourceHash(missing), to: handle)
        #expect(handle.pendingResourceHash == missing)
        #expect(imageView.image !== cachedImage)
        #expect(imageView.image != nil)

        // Changing away from the cached hash must not leave the old bitmap.
        #expect(imageView.image !== cachedImage || missing == hash)
        _ = placeholder
    }

    @Test
    func clearingOrReplacingResourceHashDropsStaleImage() throws {
        let factory = ControlFactory()
        let hashA = try ResourceHash(rawBytes: Array(repeating: 0xa1, count: 32))
        let hashB = try ResourceHash(rawBytes: Array(repeating: 0xb2, count: 32))
        let imageA = NSImage(size: NSSize(width: 3, height: 3))
        let imageB = NSImage(size: NSSize(width: 4, height: 4))

        factory.resolveResourceImage = { hash in
            if hash == hashA { return imageA }
            if hash == hashB { return imageB }
            return nil
        }

        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .image))
        let imageView = try #require(handle.view as? NSImageView)

        factory.apply(property: .resource, value: .resourceHash(hashA), to: handle)
        #expect(imageView.image === imageA)

        factory.apply(property: .resource, value: .resourceHash(hashB), to: handle)
        #expect(handle.pendingResourceHash == hashB)
        #expect(imageView.image === imageB)
        #expect(imageView.image !== imageA)

        factory.apply(property: .resource, value: nil, to: handle)
        #expect(handle.pendingResourceHash == nil)
        #expect(imageView.image !== imageA)
        #expect(imageView.image !== imageB)
        #expect(imageView.image != nil)
    }

    @Test
    func initialPropertiesAreAppliedInDeterministicOrder() {
        let node = Node(
            id: 1,
            nodeType: .toggle,
            properties: [
                .value: .bool(true),
                .selected: .bool(false),
                .label: .string("Toggle"),
                .text: .string("ignored"),
            ]
        )

        let ordered = ControlFactory.orderedPropertyEntries(of: node).map(\.0)

        #expect(ordered == ordered.sorted())
        #expect(ordered == [.label, .selected, .text, .value])
    }

    @Test
    func conflictingToggleStatePropertiesResolveDeterministically() throws {
        let handle = try ControlFactory().makeHandle(
            for: Node(
                id: 1,
                nodeType: .toggle,
                properties: [
                    .value: .bool(true),
                    .selected: .bool(false),
                ]
            )
        )

        // `value` (property 13) is applied after `selected` (property 10).
        #expect((handle.view as? NSButton)?.state == .on)
    }

    @Test
    func tableRowsHandlesDiverseValueVariants() throws {
        let modelID = ModelId(77)
        var store = SemanticStore()
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
        try store.modelInsert(
            id: modelID,
            index: 0,
            items: [
                ModelItem(itemID: ItemId(1), value: .null),
                ModelItem(itemID: ItemId(2), value: .bool(true)),
                ModelItem(itemID: ItemId(3), value: .signedInt(-42)),
                ModelItem(itemID: ItemId(4), value: .float64(3.1415)),
            ]
        )

        let node = Node(
            id: 1,
            nodeType: .table,
            properties: [
                .modelRef: .unsignedInt(modelID.value),
            ]
        )

        let factory = ControlFactory()
        let rows = factory.tableRows(for: node, store: store)

        #expect(rows.count == 4)
        #expect(rows[0].cells == [""])
        #expect(rows[1].cells == ["true"])
        #expect(rows[2].cells == ["-42"])
        #expect(rows[3].cells.first?.starts(with: "3.14") == true)
    }

    @Test
    func columnReconciliationHandlesEdgeCases() {
        let factory = ControlFactory()
        let tableView = NSTableView()

        // 1. Initial 0 columns creates 1 fallback column
        factory.reconcileColumns(in: tableView, columns: nil, fallbackTitle: "Fallback Table")
        #expect(tableView.tableColumns.count == 1)
        #expect(tableView.tableColumns[0].title == "Fallback Table")
        #expect(tableView.tableColumns[0].identifier == TableCollectionAdapter.primaryColumnIdentifier)

        // 2. Grow to 5 columns
        factory.reconcileColumns(in: tableView, columns: ["C1", "C2", "C3", "C4", "C5"], fallbackTitle: "Ignored")
        #expect(tableView.tableColumns.count == 5)
        #expect(tableView.tableColumns[0].title == "C1")
        #expect(tableView.tableColumns[0].identifier == TableCollectionAdapter.columnIdentifier(for: "C1"))
        #expect(tableView.tableColumns[4].title == "C5")
        #expect(tableView.tableColumns[4].identifier == TableCollectionAdapter.columnIdentifier(for: "C5"))

        // 3. Shrink to 0 (back to 1 fallback column)
        factory.reconcileColumns(in: tableView, columns: [], fallbackTitle: "Reset Fallback")
        #expect(tableView.tableColumns.count == 1)
        #expect(tableView.tableColumns[0].title == "Reset Fallback")
    }

    @Test
    func toggleEmitsAlternatingValueChangedInteractions() throws {
        let factory = ControlFactory()
        var receivedValues: [Bool] = []
        factory.onInteraction = { interaction in
            if case .valueChanged(nodeID: 99, value: .bool(let b)) = interaction {
                receivedValues.append(b)
            }
        }

        let handle = try factory.makeHandle(for: Node(id: 99, nodeType: .toggle))
        let button = try #require(handle.view as? NSButton)
        let trampoline = try #require(handle.actionTrampoline as? ActionTrampoline)

        button.state = .on
        trampoline.performToggleAction(button)
        button.state = .off
        trampoline.performToggleAction(button)
        button.state = .on
        trampoline.performToggleAction(button)

        #expect(receivedValues == [true, false, true])
    }

    @Test
    func cachedResourceResolvesImmediatelyOnApply() throws {
        let factory = ControlFactory()
        let hash = try ResourceHash(rawBytes: Array(repeating: 0x11, count: 32))
        let resolved = NSImage(size: NSSize(width: 2, height: 2))
        factory.resolveResourceImage = { candidate in
            candidate == hash ? resolved : nil
        }

        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .image))
        factory.apply(property: .resource, value: .resourceHash(hash), to: handle)

        let imageView = try #require(handle.view as? NSImageView)
        #expect(handle.pendingResourceHash == hash)
        #expect(imageView.image === resolved)
    }

    @Test
    func missingResourceKeepsPlaceholder() throws {
        let factory = ControlFactory()
        let hash = try ResourceHash(rawBytes: Array(repeating: 0x22, count: 32))
        factory.resolveResourceImage = { _ in nil }

        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .image))
        let before = (handle.view as? NSImageView)?.image
        factory.apply(property: .resource, value: .resourceHash(hash), to: handle)
        let after = (handle.view as? NSImageView)?.image

        #expect(handle.pendingResourceHash == hash)
        #expect(after != nil)
        // Placeholder must remain (same system symbol identity class, not a resolved bitmap).
        #expect(before != nil)
        #expect(after?.size == before?.size)
    }

    @Test
    func changingResourceHashDropsStaleResolvedImage() throws {
        let factory = ControlFactory()
        let first = try ResourceHash(rawBytes: Array(repeating: 0x33, count: 32))
        let second = try ResourceHash(rawBytes: Array(repeating: 0x44, count: 32))
        let firstImage = NSImage(size: NSSize(width: 3, height: 3))
        let secondImage = NSImage(size: NSSize(width: 4, height: 4))
        factory.resolveResourceImage = { hash in
            if hash == first { return firstImage }
            if hash == second { return secondImage }
            return nil
        }

        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .image))
        factory.apply(property: .resource, value: .resourceHash(first), to: handle)
        #expect((handle.view as? NSImageView)?.image === firstImage)

        factory.apply(property: .resource, value: .resourceHash(second), to: handle)
        #expect(handle.pendingResourceHash == second)
        #expect((handle.view as? NSImageView)?.image === secondImage)
    }

    @Test
    func clearingResourceHashRestoresPlaceholder() throws {
        let factory = ControlFactory()
        let hash = try ResourceHash(rawBytes: Array(repeating: 0x55, count: 32))
        let resolved = NSImage(size: NSSize(width: 5, height: 5))
        factory.resolveResourceImage = { candidate in
            candidate == hash ? resolved : nil
        }

        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .image))
        factory.apply(property: .resource, value: .resourceHash(hash), to: handle)
        #expect((handle.view as? NSImageView)?.image === resolved)

        factory.apply(property: .resource, value: nil, to: handle)
        #expect(handle.pendingResourceHash == nil)
        #expect((handle.view as? NSImageView)?.image !== resolved)
    }
}
