import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

@MainActor
struct ControlFactoryTests {
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
