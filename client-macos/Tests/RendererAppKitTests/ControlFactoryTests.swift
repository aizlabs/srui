import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

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
            #expect(handle.modelAdapter is TableCollectionAdapter)
            let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)
            #expect(adapter.rows.isEmpty)

        case .table:
            let scroll = try #require(handle.view as? NSScrollView)
            let table = try #require(scroll.documentView as? NSTableView)
            #expect(table.headerView != nil)
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
            #expect(adapter.rows == ["Alpha", "Beta"])
            let table = try #require(
                (handle.view as? NSScrollView)?.documentView as? NSTableView
            )
            #expect(table.numberOfRows == 2)

            factory.apply(
                property: .items,
                value: .list([.string("Gamma")]),
                to: handle
            )
            #expect(adapter.rows == ["Gamma"])
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
}
