import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
import Text

@MainActor
struct ControlFactoryPropertyTests {
    @Test
    func accessibilityPropertiesSetAndClearMetadata() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .button))

        factory.apply(property: .label, value: .string("Save"), to: handle)
        factory.apply(property: .accessibleDescription, value: .string("Persists changes"), to: handle)
        factory.apply(property: .valueDescription, value: .string("Ready"), to: handle)
        factory.apply(
            property: .actions,
            value: .list([.string("activate"), .signedInt(7), .string("showMenu")]),
            to: handle
        )

        #expect(handle.accessibilityMetadata.label == "Save")
        #expect(handle.accessibilityMetadata.description == "Persists changes")
        #expect(handle.accessibilityMetadata.valueDescription == "Ready")
        #expect(handle.accessibilityMetadata.actions == ["activate", "showMenu"])
        #expect((handle.view as? NSButton)?.title == "Save")

        for property in [
            PropertyRef.label, .accessibleDescription, .valueDescription, .actions,
        ] {
            factory.apply(property: property, value: nil, to: handle)
        }

        #expect(handle.accessibilityMetadata == RenderAccessibilityMetadata())
        #expect((handle.view as? NSButton)?.title == "Button")
    }

    @Test
    func surfaceLabelSetAndClearUpdatesWindowTitle() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .surface))
        let window = try #require(handle.window)

        factory.apply(property: .label, value: .string("Inspector"), to: handle)
        #expect(window.title == "Inspector")

        factory.apply(property: .label, value: nil, to: handle)
        #expect(window.title == "SRUI")
    }

    @Test
    func visibilitySetAndClearCoversAllEstablishedStates() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .text))

        factory.apply(property: .visibility, value: .enumToken(.visibilityHidden), to: handle)
        #expect(handle.view.isHidden == false)
        #expect(handle.view.alphaValue == 0)

        factory.apply(property: .visibility, value: .enumToken(.visibilityCollapsed), to: handle)
        #expect(handle.view.isHidden)
        #expect(handle.view.alphaValue == 1)

        factory.apply(property: .visibility, value: nil, to: handle)
        #expect(handle.view.isHidden == false)
        #expect(handle.view.alphaValue == 1)
    }

    @Test
    func enabledSelectedAndBusySetAndClear() throws {
        let factory = ControlFactory()
        let button = try factory.makeHandle(for: Node(id: 1, nodeType: .button))
        let toggle = try factory.makeHandle(for: Node(id: 2, nodeType: .toggle))
        let progress = try factory.makeHandle(for: Node(id: 3, nodeType: .progress))

        factory.apply(property: .enabled, value: .bool(false), to: button)
        factory.apply(property: .selected, value: .bool(true), to: toggle)
        factory.apply(property: .busy, value: .bool(true), to: progress)
        #expect((button.view as? NSButton)?.isEnabled == false)
        #expect((toggle.view as? NSButton)?.state == .on)
        #expect((progress.view as? NSProgressIndicator)?.isIndeterminate == true)

        factory.apply(property: .enabled, value: nil, to: button)
        factory.apply(property: .selected, value: nil, to: toggle)
        factory.apply(property: .busy, value: nil, to: progress)
        #expect((button.view as? NSButton)?.isEnabled == true)
        #expect((toggle.view as? NSButton)?.state == .off)
        #expect((progress.view as? NSProgressIndicator)?.isIndeterminate == false)
    }

    @Test(arguments: [TypeRef.textInput, .textArea])
    func readOnlySetAndClear(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))
        let editable: () -> Bool? = {
            if let field = handle.view as? NSTextField { return field.isEditable }
            return ((handle.view as? NSScrollView)?.documentView as? NSTextView)?.isEditable
        }

        factory.apply(property: .readOnly, value: .bool(true), to: handle)
        #expect(editable() == false)

        factory.apply(property: .readOnly, value: nil, to: handle)
        #expect(editable() == true)
    }

    @Test(arguments: [TypeRef.text, .richText, .textInput, .textArea])
    func textSetAndClear(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))

        factory.apply(property: .text, value: .string("semantic text"), to: handle)
        #expect(renderedText(in: handle) == "semantic text")

        factory.apply(property: .text, value: nil, to: handle)
        #expect(renderedText(in: handle) == "")
    }

    @Test(arguments: [TypeRef.text, .richText, .textInput, .textArea])
    func stringValueSetAndClear(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))

        factory.apply(property: .value, value: .string("semantic value"), to: handle)
        #expect(renderedText(in: handle) == "semantic value")

        factory.apply(property: .value, value: nil, to: handle)
        #expect(renderedText(in: handle) == "")
    }

    @Test(arguments: [
        Value.float64(0.25), .signedInt(1), .unsignedInt(1),
    ])
    func progressAcceptsEveryEstablishedNumericValue(value: Value) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .progress))
        let progress = try #require(handle.view as? NSProgressIndicator)

        factory.apply(property: .value, value: value, to: handle)
        let expected = value.asFloat64
            ?? value.asSignedInt.map { Double($0) }
            ?? value.asUnsignedInt.map { Double($0) }
        #expect(progress.doubleValue == expected)

        factory.apply(property: .value, value: nil, to: handle)
        #expect(progress.doubleValue == 0)
    }

    @Test
    func toggleValueSetAndClear() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .toggle))
        let toggle = try #require(handle.view as? NSButton)

        factory.apply(property: .value, value: .bool(true), to: handle)
        #expect(toggle.state == .on)
        factory.apply(property: .value, value: nil, to: handle)
        #expect(toggle.state == .off)
    }

    @Test
    func alignmentsSetAndClearMetadataAndStackAlignment() throws {
        let factory = ControlFactory()
        let column = try factory.makeHandle(for: Node(id: 1, nodeType: .column))
        let row = try factory.makeHandle(for: Node(id: 2, nodeType: .row))
        let columnStack = try #require(column.view as? NSStackView)
        let rowStack = try #require(row.view as? NSStackView)

        factory.apply(
            property: .horizontalAlignment,
            value: .enumToken(.horizontalAlignmentTrailing),
            to: column
        )
        factory.apply(
            property: .verticalAlignment,
            value: .enumToken(.verticalAlignmentBottom),
            to: row
        )
        #expect(column.layoutMetadata.horizontalAlignment == .horizontalAlignmentTrailing)
        #expect(columnStack.alignment == .trailing)
        #expect(row.layoutMetadata.verticalAlignment == .verticalAlignmentBottom)
        #expect(rowStack.alignment == .bottom)

        // A cleared alignment restores AppKit's own default for the orientation, not an arbitrary
        // edge: `NSStackView` centers on the cross axis unless told otherwise.
        factory.apply(property: .horizontalAlignment, value: nil, to: column)
        factory.apply(property: .verticalAlignment, value: nil, to: row)
        #expect(column.layoutMetadata.horizontalAlignment == nil)
        #expect(columnStack.alignment == .centerX)
        #expect(row.layoutMetadata.verticalAlignment == nil)
        #expect(rowStack.alignment == .centerY)

        factory.apply(
            property: .horizontalAlignment,
            value: .enumToken(.horizontalAlignmentLeading),
            to: column
        )
        #expect(columnStack.alignment == .leading)
        factory.apply(
            property: .verticalAlignment,
            value: .enumToken(.verticalAlignmentTop),
            to: row
        )
        #expect(rowStack.alignment == .top)
    }

    /// §4 inv. 13: an unrecognized token is not silently coerced into some other alignment.
    @Test
    func unknownAlignmentTokenLeavesStackAlignmentUnchanged() throws {
        let factory = ControlFactory()
        let column = try factory.makeHandle(for: Node(id: 1, nodeType: .column))
        let row = try factory.makeHandle(for: Node(id: 2, nodeType: .row))
        let columnStack = try #require(column.view as? NSStackView)
        let rowStack = try #require(row.view as? NSStackView)

        factory.apply(
            property: .horizontalAlignment,
            value: .enumToken(.horizontalAlignmentTrailing),
            to: column
        )
        factory.apply(
            property: .verticalAlignment,
            value: .enumToken(.verticalAlignmentBottom),
            to: row
        )
        #expect(columnStack.alignment == .trailing)
        #expect(rowStack.alignment == .bottom)

        // Unassigned value in the alignment enums, and a token from the wrong alignment family.
        factory.apply(
            property: .horizontalAlignment,
            value: .enumToken(EnumToken(enumID: 9, valueID: 99)),
            to: column
        )
        factory.apply(
            property: .verticalAlignment,
            value: .enumToken(.horizontalAlignmentLeading),
            to: row
        )
        #expect(columnStack.alignment == .trailing)
        #expect(rowStack.alignment == .bottom)
    }

    /// A `value` of the wrong type is not content this control can show; wiping the rendered text
    /// would turn a type mismatch into visible data loss (§4 inv. 13).
    @Test(arguments: [TypeRef.textInput, .textArea])
    func nonStringValueLeavesTextUnchanged(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))

        factory.apply(property: .value, value: .string("typed"), to: handle)
        factory.apply(property: .value, value: .signedInt(42), to: handle)

        if let field = handle.view as? NSTextField {
            #expect(field.stringValue == "typed")
            factory.apply(property: .value, value: nil, to: handle)
            #expect(field.stringValue == "")
        } else if let textView = (handle.view as? NSScrollView)?.documentView as? NSTextView {
            #expect(textView.string == "typed")
            factory.apply(property: .value, value: nil, to: handle)
            #expect(textView.string == "")
        } else {
            Issue.record("unexpected view for \(nodeType)")
        }
    }

    @Test(arguments: [TypeRef.textInput, .textArea])
    func textEditorsRetainANativeAdapter(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))
        #expect(handle.textAdapter != nil)
        #expect(handle.textAdapter?.nodeID == NodeId(1))
    }

    @Test(arguments: [TypeRef.textInput, .textArea])
    func valueWinsOverTextOnEditors(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(
            for: Node(
                id: 1,
                nodeType: nodeType,
                properties: [
                    (.text, .string("from-text")),
                    (.value, .string("from-value")),
                ]
            )
        )
        #expect(renderedText(in: handle) == "from-value")
        #expect(factory.textEditingSession.localValue(for: 1) == "from-value")
    }

    @Test(arguments: [TypeRef.textInput, .textArea])
    func identicalAuthoritativeStringDoesNotResetNativeText(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))
        factory.apply(property: .value, value: .string("stable"), to: handle)
        let before = renderedText(in: handle)
        factory.apply(property: .value, value: .string("stable"), to: handle)
        #expect(renderedText(in: handle) == before)
        #expect(renderedText(in: handle) == "stable")
    }

    @Test(arguments: [TypeRef.textInput, .textArea])
    func validationStateDecoratesEditors(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))
        let adapter = try #require(handle.textAdapter)

        factory.apply(property: .validationState, value: .enumToken(StandardValidationState.valid.enumToken), to: handle)
        #expect(adapter.validationState == .valid)

        factory.apply(property: .validationState, value: .enumToken(StandardValidationState.warning.enumToken), to: handle)
        #expect(adapter.validationState == .warning)

        factory.apply(property: .validationState, value: .enumToken(StandardValidationState.error.enumToken), to: handle)
        #expect(adapter.validationState == .error)

        factory.apply(property: .validationState, value: nil, to: handle)
        #expect(adapter.validationState == nil)
    }

    @Test
    func growAndShrinkSetAndClearPriorities() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .text))

        factory.apply(property: .grow, value: .bool(true), to: handle)
        factory.apply(property: .shrink, value: .bool(true), to: handle)
        #expect(handle.view.contentHuggingPriority(for: .horizontal) == .defaultLow)
        #expect(handle.view.contentHuggingPriority(for: .vertical) == .defaultLow)
        #expect(handle.view.contentCompressionResistancePriority(for: .horizontal) == .defaultLow)
        #expect(handle.view.contentCompressionResistancePriority(for: .vertical) == .defaultLow)

        factory.apply(property: .grow, value: nil, to: handle)
        factory.apply(property: .shrink, value: nil, to: handle)
        #expect(handle.view.contentHuggingPriority(for: .horizontal) == .defaultHigh)
        #expect(handle.view.contentHuggingPriority(for: .vertical) == .defaultHigh)
        #expect(handle.view.contentCompressionResistancePriority(for: .horizontal) == .defaultHigh)
        #expect(handle.view.contentCompressionResistancePriority(for: .vertical) == .defaultHigh)
    }

    @Test(arguments: [
        (PropertyRef.minimumSize, NSLayoutConstraint.Relation.greaterThanOrEqual),
        (PropertyRef.maximumSize, NSLayoutConstraint.Relation.lessThanOrEqual),
        (PropertyRef.preferredSize, NSLayoutConstraint.Relation.equal),
    ])
    func sizePropertiesReplaceAndClearConstraints(
        property: PropertyRef,
        relation: NSLayoutConstraint.Relation
    ) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .text))
        let first = Size(width: 120, height: 44)
        let replacement = Size(width: 80, height: 24)

        factory.apply(property: property, value: .size(first), to: handle)
        let firstConstraints = try #require(handle.propertyConstraints[property])
        #expect(firstConstraints.count == 2)
        #expect(firstConstraints.allSatisfy { $0.isActive })
        #expect(firstConstraints.allSatisfy { $0.relation == relation })
        #expect(firstConstraints.map(\.constant) == [120, 44])

        factory.apply(property: property, value: .size(replacement), to: handle)
        let replacementConstraints = try #require(handle.propertyConstraints[property])
        #expect(firstConstraints.allSatisfy { $0.isActive == false })
        #expect(replacementConstraints.map(\.constant) == [80, 24])
        #expect(sizeMetadata(for: property, in: handle) == replacement)

        factory.apply(property: property, value: nil, to: handle)
        #expect(replacementConstraints.allSatisfy { $0.isActive == false })
        #expect(handle.propertyConstraints[property]?.isEmpty == true)
        #expect(sizeMetadata(for: property, in: handle) == nil)
    }

    @Test(arguments: [
        (EnumToken.textRoleTitle, CGFloat(24)),
        (.textRoleHeading, CGFloat(18)),
        (.textRoleBody, CGFloat(13)),
        (.textRoleCaption, CGFloat(11)),
        (.textRoleCode, CGFloat(13)),
        (.textRoleStatus, CGFloat(13)),
        (.textRoleWarning, CGFloat(13)),
        (.textRoleError, CGFloat(13)),
    ])
    func everyEstablishedTextRoleIsRendered(token: EnumToken, pointSize: CGFloat) throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .text))
        let label = try #require(handle.view as? NSTextField)

        factory.apply(property: .role, value: .enumToken(token), to: handle)
        #expect(label.font?.pointSize == pointSize)
    }

    @Test
    func everyEstablishedActionRoleIsRendered() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(for: Node(id: 1, nodeType: .button))
        let button = try #require(handle.view as? NSButton)

        factory.apply(property: .role, value: .enumToken(.actionRolePrimary), to: handle)
        #expect(button.bezelStyle == .rounded)
        #expect(button.contentTintColor == .controlAccentColor)
        #expect(button.keyEquivalent == "\r")

        factory.apply(property: .role, value: .enumToken(.actionRoleDestructive), to: handle)
        #expect(button.contentTintColor == .systemRed)
        #expect(button.keyEquivalent == "")

        factory.apply(property: .role, value: .enumToken(.actionRoleQuiet), to: handle)
        #expect(button.bezelStyle == .inline)
        #expect(button.contentTintColor == nil)

        factory.apply(property: .role, value: .enumToken(.actionRoleNormal), to: handle)
        #expect(button.bezelStyle == .rounded)
        #expect(button.contentTintColor == nil)
        #expect(button.keyEquivalent == "")
    }

    private func renderedText(in handle: RenderHandle) -> String? {
        if let field = handle.view as? NSTextField { return field.stringValue }
        if let textView = handle.view as? NSTextView { return textView.string }
        return ((handle.view as? NSScrollView)?.documentView as? NSTextView)?.string
    }

    private func sizeMetadata(for property: PropertyRef, in handle: RenderHandle) -> Size? {
        switch property {
        case .minimumSize: handle.layoutMetadata.minimumSize
        case .maximumSize: handle.layoutMetadata.maximumSize
        case .preferredSize: handle.layoutMetadata.preferredSize
        default: nil
        }
    }
}
