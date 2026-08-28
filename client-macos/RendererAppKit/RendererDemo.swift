import AppKit
import SemanticModel

@MainActor
public enum RendererDemo {
    private static var retainedDelegate: DemoApplicationDelegate?

    public static func run() -> Never {
        RendererDiagnostics.log("demo entering RendererDemo.run")
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)

        let delegate = DemoApplicationDelegate()
        retainedDelegate = delegate
        application.delegate = delegate
        RendererDiagnostics.log("demo finishing NSApplication launch")
        application.finishLaunching()
        RendererDiagnostics.log("demo mounting initial canned transaction")
        delegate.start()
        RendererDiagnostics.log("demo entering NSApplication run loop")
        application.run()
        fatalError("NSApplication run loop exited")
    }
}

@MainActor
private final class DemoApplicationDelegate: NSObject, NSApplicationDelegate {
    private let renderer = AppKitRenderer()
    private let applier = TransactionApplier()

    func start() {
        let transaction = DemoFixtures.initial(
            baseRevision: applier.currentSnapshot.revision
        )
        guard case .success = applier.apply(record: transaction) else {
            fatalError("Initial renderer demo transaction failed")
        }
        let committedSnapshot = applier.currentSnapshot

        do {
            try renderer.attach(store: committedSnapshot.store)
            renderer.showWindows()
            logFrames(phase: "initial")
        } catch {
            fatalError("Initial renderer demo mount failed: \(error)")
        }

        NSApplication.shared.activate(ignoringOtherApps: true)
        Timer.scheduledTimer(
            timeInterval: 1.5,
            target: self,
            selector: #selector(applyScalarFixture),
            userInfo: nil,
            repeats: false
        )
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    @objc
    private func applyScalarFixture() {
        let observedIDs: [NodeId] = [
            DemoFixtures.progressID,
            DemoFixtures.titleID,
            DemoFixtures.buttonID,
            DemoFixtures.toggleID,
        ]
        let identities = Dictionary(uniqueKeysWithValues: observedIDs.compactMap { nodeID in
            renderer.registry.view(for: nodeID).map { (nodeID, ObjectIdentifier($0)) }
        })

        let transaction = DemoFixtures.scalarUpdate(
            baseRevision: applier.currentSnapshot.revision
        )
        guard case .success = applier.apply(record: transaction) else {
            fatalError("Scalar renderer demo transaction failed")
        }
        let committedSnapshot = applier.currentSnapshot

        do {
            try renderer.apply(
                transaction: transaction,
                newStore: committedSnapshot.store
            )
        } catch {
            fatalError("Scalar renderer demo update failed: \(error)")
        }

        for (nodeID, identity) in identities {
            guard let view = renderer.registry.view(for: nodeID) else {
                fatalError("Scalar update removed render handle \(nodeID)")
            }
            precondition(
                ObjectIdentifier(view) == identity,
                "Scalar update rebuilt render handle \(nodeID)"
            )
        }
        RendererDiagnostics.log("demo scalar transaction updated observed controls in place")
        logFrames(phase: "scalar")
    }

    private func logFrames(phase: String) {
        for handle in renderer.registry.allHandles {
            RendererDiagnostics.log(
                "demo \(phase) node=\(handle.nodeID) "
                    + "type=\(handle.nodeType) frame=\(NSStringFromRect(handle.view.frame)) "
                    + "hidden=\(handle.view.isHidden) "
                    + "superview=\(handle.view.superview.map { String(describing: type(of: $0)) } ?? "nil")"
            )
        }
    }
}

private enum DemoFixtures {
    static let surfaceID = NodeId(1)
    static let scrollID = NodeId(2)
    static let columnID = NodeId(3)
    static let titleID = NodeId(4)
    static let richTextID = NodeId(5)
    static let rowID = NodeId(6)
    static let buttonID = NodeId(7)
    static let toggleID = NodeId(8)
    static let spacerID = NodeId(9)
    static let progressID = NodeId(10)
    static let separatorID = NodeId(11)
    static let textInputID = NodeId(12)
    static let textAreaID = NodeId(13)
    static let gridID = NodeId(14)
    static let gridTextOneID = NodeId(15)
    static let gridTextTwoID = NodeId(16)
    static let imageID = NodeId(17)
    static let listID = NodeId(18)
    static let tableID = NodeId(19)
    static let treeID = NodeId(20)
    static let statusID = NodeId(21)

    static func initial(baseRevision: Revision) -> Transaction {
        Transaction(
            baseRevision: baseRevision,
            operations: [
                .createNode(
                    id: surfaceID,
                    nodeType: .surface,
                    properties: [
                        Property(property: .label, value: .string("SRUI Required-Tier AppKit Renderer"))
                    ]
                ),
                .createNode(id: scrollID, nodeType: .scroll, parentID: surfaceID),
                .createNode(
                    id: columnID,
                    nodeType: .column,
                    parentID: scrollID,
                    properties: [
                        Property(property: .spacingRole, value: .enumToken(.spacingRoleNormal)),
                        Property(property: .paddingRole, value: .enumToken(.paddingRoleTight))
                    ]
                ),
                .createNode(
                    id: titleID,
                    nodeType: .text,
                    parentID: columnID,
                    properties: [
                        Property(property: .text, value: .string("Required-tier native widgets")),
                        Property(property: .role, value: .enumToken(.textRoleTitle))
                    ]
                ),
                .createNode(
                    id: richTextID,
                    nodeType: .richText,
                    parentID: columnID,
                    properties: [
                        Property(
                            property: .text,
                            value: .string("RichText is selectable and rendered by NSTextView. Hover, focus, selection, and editing feedback remain local AppKit behavior.")
                        )
                    ]
                ),
                .createNode(id: rowID, nodeType: .row, parentID: columnID),
                .createNode(
                    id: buttonID,
                    nodeType: .button,
                    parentID: rowID,
                    properties: [
                        Property(property: .label, value: .string("Native Button")),
                        Property(property: .role, value: .enumToken(.actionRolePrimary))
                    ]
                ),
                .createNode(
                    id: toggleID,
                    nodeType: .toggle,
                    parentID: rowID,
                    properties: [
                        Property(property: .label, value: .string("Native Toggle")),
                        Property(property: .value, value: .bool(false))
                    ]
                ),
                .createNode(
                    id: spacerID,
                    nodeType: .spacer,
                    parentID: rowID,
                    properties: [
                        Property(
                            property: .minimumSize,
                            value: .size(Size(width: 24, height: 1))
                        ),
                        Property(property: .grow, value: .bool(true))
                    ]
                ),
                .createNode(
                    id: progressID,
                    nodeType: .progress,
                    parentID: rowID,
                    properties: [
                        Property(property: .value, value: .float64(0.25))
                    ]
                ),
                .createNode(id: separatorID, nodeType: .separator, parentID: columnID),
                .createNode(
                    id: textInputID,
                    nodeType: .textInput,
                    parentID: columnID,
                    properties: [
                        Property(property: .placeholder, value: .string("TextInput (local caret and focus)"))
                    ]
                ),
                .createNode(
                    id: textAreaID,
                    nodeType: .textArea,
                    parentID: columnID,
                    properties: [
                        Property(property: .text, value: .string("TextArea uses NSTextView inside NSScrollView."))
                    ]
                ),
                .createNode(id: gridID, nodeType: .grid, parentID: columnID),
                .createNode(
                    id: gridTextOneID,
                    nodeType: .text,
                    parentID: gridID,
                    properties: [
                        Property(property: .text, value: .string("Grid row one"))
                    ]
                ),
                .createNode(
                    id: gridTextTwoID,
                    nodeType: .text,
                    parentID: gridID,
                    properties: [
                        Property(property: .text, value: .string("Grid row two"))
                    ]
                ),
                .createNode(
                    id: imageID,
                    nodeType: .image,
                    parentID: columnID,
                    properties: [
                        Property(property: .label, value: .string("Image placeholder"))
                    ]
                ),
                .createNode(
                    id: listID,
                    nodeType: .list,
                    parentID: columnID,
                    properties: [
                        Property(
                            property: .items,
                            value: .list([.string("List item A"), .string("List item B")])
                        )
                    ]
                ),
                .createNode(
                    id: tableID,
                    nodeType: .table,
                    parentID: columnID,
                    properties: [
                        Property(property: .label, value: .string("Table column")),
                        Property(
                            property: .items,
                            value: .list([.string("Table row 1"), .string("Table row 2")])
                        )
                    ]
                ),
                .createNode(
                    id: treeID,
                    nodeType: .tree,
                    parentID: columnID,
                    properties: [
                        Property(
                            property: .items,
                            value: .list([.string("Tree item 1"), .string("Tree item 2")])
                        )
                    ]
                ),
                .createNode(
                    id: statusID,
                    nodeType: .text,
                    parentID: columnID,
                    properties: [
                        Property(
                            property: .text,
                            value: .string("A scalar fixture transaction will run after 1.5 seconds.")
                        ),
                        Property(property: .role, value: .enumToken(.textRoleStatus))
                    ]
                ),
            ]
        )
    }

    static func scalarUpdate(baseRevision: Revision) -> Transaction {
        Transaction(
            baseRevision: baseRevision,
            operations: [
                .setProperty(id: progressID, property: .value, value: .float64(0.82)),
                .setProperty(
                    id: titleID,
                    property: .text,
                    value: .string("Required-tier widgets — updated in place")
                ),
                .setProperty(
                    id: buttonID,
                    property: .label,
                    value: .string("Updated Button")
                ),
                .setProperty(id: toggleID, property: .value, value: .bool(true)),
                .setProperty(
                    id: statusID,
                    property: .text,
                    value: .string("Scalar SET_PROPERTY preserved every observed NSView identity.")
                ),
            ]
        )
    }
}
