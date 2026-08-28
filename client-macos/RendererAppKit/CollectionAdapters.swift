import AppKit
import SemanticModel

@MainActor
final class TableCollectionAdapter: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private(set) var rows: [String]

    init(rows: [String]) {
        self.rows = rows
    }

    func update(rows: [String], tableView: NSTableView) {
        self.rows = rows
        tableView.reloadData()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        rows.count
    }

    func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        NSTextField(labelWithString: rows[row])
    }
}

@MainActor
final class OutlineCollectionAdapter: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
    final class Item: NSObject {
        let title: String

        init(title: String) {
            self.title = title
        }
    }

    private(set) var items: [Item]

    init(rows: [String]) {
        self.items = rows.map(Item.init(title:))
    }

    func update(rows: [String], outlineView: NSOutlineView) {
        items = rows.map(Item.init(title:))
        outlineView.reloadData()
    }

    func outlineView(
        _ outlineView: NSOutlineView,
        numberOfChildrenOfItem item: Any?
    ) -> Int {
        item == nil ? items.count : 0
    }

    func outlineView(
        _ outlineView: NSOutlineView,
        child index: Int,
        ofItem item: Any?
    ) -> Any {
        items[index]
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        false
    }

    func outlineView(
        _ outlineView: NSOutlineView,
        viewFor tableColumn: NSTableColumn?,
        item: Any
    ) -> NSView? {
        guard let item = item as? Item else { return nil }
        return NSTextField(labelWithString: item.title)
    }
}
