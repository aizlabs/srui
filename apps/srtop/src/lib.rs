//! Read-only Process Explorer shell using existing SRUI widgets and transactions
//! (design §§6–8, 12, 22, 29; PX-001/PX-002/PX-003/PX-004, PX-007 system
//! summary, PX-009 server-authoritative sorting, PX-010 filtering, PX-068
//! column chooser). No process is ever opened for control: the only
//! client-input handlers are the sort buttons', the filter input's and the
//! column chooser buttons', which reorder, filter or re-lay out the published
//! rows and touch no process.

pub mod catalogue;
pub mod filter;
pub mod metric;
pub mod on_demand;
pub mod procfs;
mod projection;
pub mod refresh;
pub mod sort;
pub mod source;
pub mod summary;

use srui_sdk::*;
use srui_sessiond::{Session, SessionError};

pub const SURFACE: NodeId = NodeId::new(1);
pub const COLUMN: NodeId = NodeId::new(2);
pub const HEADING: NodeId = NodeId::new(3);
pub const STATUS: NodeId = NodeId::new(4);
pub const TABLE: NodeId = NodeId::new(5);
/// The filter controls' row, between the system summary and the sort controls (PX-010).
pub const FILTER_ROW: NodeId = NodeId::new(25);
/// The native filter text field (PX-010).
pub const FILTER_INPUT: NodeId = NodeId::new(26);
/// The filter pending/committed status caption (PX-010).
pub const FILTER_STATUS: NodeId = NodeId::new(27);
pub const FILTER_LABEL: &str = "Filter processes";
pub const FILTER_PLACEHOLDER: &str = "Filter by name or command (| for OR)";
pub const FILTER_DESCRIPTION: &str =
    "Filters visible table rows on the server by process name or available command text. Separate OR terms with |.";
pub const UNCOLLECTED_FILTER_STATUS: &str = "Filter disabled: process collection not started";
pub const FILTER_NODE_COUNT: usize = 3;
/// The sort controls' row, between the system summary and the table (PX-009).
pub const SORT_ROW: NodeId = NodeId::new(20);
/// One sort button per catalogued column, in column order.
pub const SORT_PID: NodeId = NodeId::new(21);
pub const SORT_NAME: NodeId = NodeId::new(22);
pub const SORT_RESIDENT: NodeId = NodeId::new(23);
pub const SORT_CPU: NodeId = NodeId::new(24);
/// Every sort button with the column it orders, in column order (PX-009).
pub const SORT_BUTTONS: [(NodeId, catalogue::ProcessField); 4] = [
    (SORT_PID, catalogue::ProcessField::Pid),
    (SORT_NAME, catalogue::ProcessField::Name),
    (SORT_RESIDENT, catalogue::ProcessField::Resident),
    (SORT_CPU, catalogue::ProcessField::Cpu),
];
/// The column chooser's row, between the sort controls and the table (PX-068).
pub const COLUMN_CHOOSER_ROW: NodeId = NodeId::new(28);
/// Per catalogued column, in column order: one button that hides or shows it,
/// and two that move it one place earlier or later among the visible columns.
pub const COLUMN_TOGGLE_PID: NodeId = NodeId::new(29);
pub const COLUMN_EARLIER_PID: NodeId = NodeId::new(30);
pub const COLUMN_LATER_PID: NodeId = NodeId::new(31);
pub const COLUMN_TOGGLE_NAME: NodeId = NodeId::new(32);
pub const COLUMN_EARLIER_NAME: NodeId = NodeId::new(33);
pub const COLUMN_LATER_NAME: NodeId = NodeId::new(34);
pub const COLUMN_TOGGLE_RESIDENT: NodeId = NodeId::new(35);
pub const COLUMN_EARLIER_RESIDENT: NodeId = NodeId::new(36);
pub const COLUMN_LATER_RESIDENT: NodeId = NodeId::new(37);
pub const COLUMN_TOGGLE_CPU: NodeId = NodeId::new(38);
pub const COLUMN_EARLIER_CPU: NodeId = NodeId::new(39);
pub const COLUMN_LATER_CPU: NodeId = NodeId::new(40);
/// The button that restores the default columns (PX-068).
pub const COLUMNS_RESTORE: NodeId = NodeId::new(41);

/// What one column chooser button does (PX-068). Each is resolved against the
/// layout the server holds when the click is applied, never against anything
/// the client supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAction {
    /// Hide the column when it is shown, show it when it is hidden.
    Toggle(catalogue::ProcessField),
    /// Swap the column with the visible column before it.
    Earlier(catalogue::ProcessField),
    /// Swap the column with the visible column after it.
    Later(catalogue::ProcessField),
    /// Bring back the catalogued default layout.
    RestoreDefaults,
}

impl ColumnAction {
    /// The layout this action turns `layout` into.
    pub fn apply(self, layout: &catalogue::ColumnLayout) -> catalogue::ColumnLayout {
        match self {
            Self::Toggle(field) if layout.shows(field) => layout.hiding(field),
            Self::Toggle(field) => layout.showing(field),
            Self::Earlier(field) => layout.moving_earlier(field),
            Self::Later(field) => layout.moving_later(field),
            Self::RestoreDefaults => catalogue::ColumnLayout::default(),
        }
    }

    /// The button's label under `layout`: what a click would do, naming the
    /// column by its short name.
    pub fn label(self, layout: &catalogue::ColumnLayout) -> String {
        match self {
            Self::Toggle(field) if layout.shows(field) => {
                format!("Hide {} column", field.button_name())
            }
            Self::Toggle(field) => format!("Show {} column", field.button_name()),
            Self::Earlier(field) => format!("Move {} column left", field.button_name()),
            Self::Later(field) => format!("Move {} column right", field.button_name()),
            Self::RestoreDefaults => "Restore default columns".to_string(),
        }
    }

    /// Whether a click would change `layout`: a control that cannot is
    /// disabled, so event validation refuses it (§7.4).
    pub fn enabled(self, layout: &catalogue::ColumnLayout) -> bool {
        match self {
            Self::Toggle(field) => !layout.shows(field) || layout.can_hide(field),
            Self::Earlier(field) => layout.can_move_earlier(field),
            Self::Later(field) => layout.can_move_later(field),
            Self::RestoreDefaults => !layout.is_default(),
        }
    }
}

/// Every column chooser button with what it does, in published order (PX-068).
pub const COLUMN_CHOOSER_BUTTONS: [(NodeId, ColumnAction); 13] = {
    use catalogue::ProcessField::*;
    use ColumnAction::*;
    [
        (COLUMN_TOGGLE_PID, Toggle(Pid)),
        (COLUMN_EARLIER_PID, Earlier(Pid)),
        (COLUMN_LATER_PID, Later(Pid)),
        (COLUMN_TOGGLE_NAME, Toggle(Name)),
        (COLUMN_EARLIER_NAME, Earlier(Name)),
        (COLUMN_LATER_NAME, Later(Name)),
        (COLUMN_TOGGLE_RESIDENT, Toggle(Resident)),
        (COLUMN_EARLIER_RESIDENT, Earlier(Resident)),
        (COLUMN_LATER_RESIDENT, Later(Resident)),
        (COLUMN_TOGGLE_CPU, Toggle(Cpu)),
        (COLUMN_EARLIER_CPU, Earlier(Cpu)),
        (COLUMN_LATER_CPU, Later(Cpu)),
        (COLUMNS_RESTORE, RestoreDefaults),
    ]
};
pub const COLUMN_CHOOSER_NODE_COUNT: usize = 1 + COLUMN_CHOOSER_BUTTONS.len();

pub const MODEL: ModelId = ModelId::new(1);
pub const TITLE: &str = "Process Explorer";
pub const FIXTURE_TITLE: &str = "Process Explorer — title fixture";
pub const STATUS_TEXT: &str = "Read-only · Process collection not started";
/// Every node of the shell: the five above, the fourteen of the system summary
/// (PX-007), which sits between the status line and the filter controls, the three
/// of the filter bar (PX-010), the sort row with its four buttons (PX-009),
/// and the column chooser's row with its buttons (PX-068). Rows are model
/// items, never nodes.
pub const SHELL_NODE_COUNT: usize = 5
    + summary::NODE_COUNT
    + FILTER_NODE_COUNT
    + 1
    + SORT_BUTTONS.len()
    + COLUMN_CHOOSER_NODE_COUNT;

/// The sort button that orders `field`.
pub fn sort_button_of(field: catalogue::ProcessField) -> NodeId {
    match field {
        catalogue::ProcessField::Pid => SORT_PID,
        catalogue::ProcessField::Name => SORT_NAME,
        catalogue::ProcessField::Resident => SORT_RESIDENT,
        catalogue::ProcessField::Cpu => SORT_CPU,
    }
}

/// Installs the four sort buttons' `ACTIVATE` handlers (§7.6, PX-009).
///
/// A click is applied before the event is acknowledged: the handler reorders
/// `view` and publishes the result, so the acknowledgement's revision already
/// holds the new order (§18.2). Clicks are applied one at a time, in the order
/// the session dispatches them, each against the order the previous one left,
/// and the refresh loop publishes every later sample in that order. Nothing
/// here reads a PID, a row or any argument the client supplied: the column is
/// the button's, fixed when the handler is installed. These are the only
/// client-input handlers srtop installs.
pub fn install_sort_handlers(session: &Session, view: &refresh::SharedView) {
    let pid = std::sync::Arc::clone(view);
    session.on(SORT_PID, ACTIVATE, move |session, _| {
        sort_clicked(&pid, session, catalogue::ProcessField::Pid);
    });
    let name = std::sync::Arc::clone(view);
    session.on(SORT_NAME, ACTIVATE, move |session, _| {
        sort_clicked(&name, session, catalogue::ProcessField::Name);
    });
    let resident = std::sync::Arc::clone(view);
    session.on(SORT_RESIDENT, ACTIVATE, move |session, _| {
        sort_clicked(&resident, session, catalogue::ProcessField::Resident);
    });
    let cpu = std::sync::Arc::clone(view);
    session.on(SORT_CPU, ACTIVATE, move |session, _| {
        sort_clicked(&cpu, session, catalogue::ProcessField::Cpu);
    });
}

/// Installs the column chooser buttons' `ACTIVATE` handlers (§7.6, PX-068).
///
/// Like a sort click, a chooser click is applied before the event is
/// acknowledged, so the acknowledgement's revision already holds the new
/// headings and cells. The action is the button's, fixed here; nothing the
/// client sent is read, no process is touched, and the rows keep their item
/// IDs, order and active sort.
pub fn install_column_handlers(session: &Session, view: &refresh::SharedView) {
    use catalogue::ProcessField::*;
    use ColumnAction::*;
    let handler = |action: ColumnAction| {
        let view = std::sync::Arc::clone(view);
        move |session: &Session, _: &srui_protocol::Event| column_clicked(&view, session, action)
    };
    session.on(COLUMN_TOGGLE_PID, ACTIVATE, handler(Toggle(Pid)));
    session.on(COLUMN_EARLIER_PID, ACTIVATE, handler(Earlier(Pid)));
    session.on(COLUMN_LATER_PID, ACTIVATE, handler(Later(Pid)));
    session.on(COLUMN_TOGGLE_NAME, ACTIVATE, handler(Toggle(Name)));
    session.on(COLUMN_EARLIER_NAME, ACTIVATE, handler(Earlier(Name)));
    session.on(COLUMN_LATER_NAME, ACTIVATE, handler(Later(Name)));
    session.on(COLUMN_TOGGLE_RESIDENT, ACTIVATE, handler(Toggle(Resident)));
    session.on(
        COLUMN_EARLIER_RESIDENT,
        ACTIVATE,
        handler(Earlier(Resident)),
    );
    session.on(COLUMN_LATER_RESIDENT, ACTIVATE, handler(Later(Resident)));
    session.on(COLUMN_TOGGLE_CPU, ACTIVATE, handler(Toggle(Cpu)));
    session.on(COLUMN_EARLIER_CPU, ACTIVATE, handler(Earlier(Cpu)));
    session.on(COLUMN_LATER_CPU, ACTIVATE, handler(Later(Cpu)));
    session.on(COLUMNS_RESTORE, ACTIVATE, handler(RestoreDefaults));
}

/// One click on a column chooser button.
fn column_clicked(
    view: &std::sync::Mutex<refresh::ProcessView>,
    session: &Session,
    action: ColumnAction,
) {
    if let Err(error) = refresh::lock_view(view).click_column(session, action) {
        // As for a sort click: the view still holds what committed, and the
        // next click applies against that.
        eprintln!("srtop: column layout not published: {error}");
    }
}

/// Installs the filter input's `TEXT_EDIT` handler (§7.6, §22.6, PX-010).
pub fn install_filter_handlers(session: &Session, view: &refresh::SharedView) {
    let view = std::sync::Arc::clone(view);
    session.on(FILTER_INPUT, TEXT_EDIT, move |session, event| {
        refresh::filter_edited(&view, session, event);
    });
}

/// Alias for [`install_filter_handlers`].
pub fn register_filter_handlers(session: &Session, view: &refresh::SharedView) {
    install_filter_handlers(session, view);
}

/// One click on `field`'s sort button.
fn sort_clicked(
    view: &std::sync::Mutex<refresh::ProcessView>,
    session: &Session,
    field: catalogue::ProcessField,
) {
    if let Err(error) = refresh::lock_view(view).click_sort(session, field) {
        // A failed transaction leaves the store unchanged and the view holding
        // what the store holds; the click is acknowledged and the next one
        // applies against the order that did commit.
        eprintln!("srtop: sort not published: {error}");
    }
}

/// Creates the complete empty shell in one authoritative commit. It has no
/// source, so its summary says that nothing was sampled and its sort buttons
/// are disabled: there is no collection for them to order.
pub fn initialize(session: &Session) -> Result<(), SessionError> {
    let summary = summary::properties(&summary::summarize(&summary::State::NotStarted));
    initialize_rows(session, vec![], STATUS_TEXT, &summary, false)
}

/// Samples the injected source once and publishes its model rows atomically with the shell,
/// labeling the status with the source's own truthful description rather than a fixed
/// fixture string. No process actions are installed.
///
/// The returned view is what a later refresh diffs against; a caller that only
/// wants the one-shot publication can use [`initialize_from_source`].
pub fn start_from_source(
    session: &Session,
    source: &mut impl source::ProcessSource,
) -> Result<(refresh::ProcessView, source::ProcessSnapshot), Box<dyn std::error::Error>> {
    refresh::ProcessView::start(session, source)
}

/// Publishes one snapshot and discards the refresh state.
pub fn initialize_from_source(
    session: &Session,
    source: &mut impl source::ProcessSource,
) -> Result<source::ProcessSnapshot, Box<dyn std::error::Error>> {
    start_from_source(session, source).map(|(_, snapshot)| snapshot)
}

/// The published status line: the source's own truthful description, plus an
/// explicit clause for each way the scan fell short. A complete snapshot is
/// labeled exactly as the source describes itself, so an authoritative empty
/// result and a degraded scan are never the same text and a partial list is
/// never presented as the whole picture (§22.1).
///
/// Each degradation states what actually happened rather than one fixed clause.
/// A root that could not be listed publishes its reason and no record count,
/// because "0 unreadable" there would claim the opposite of the truth: nothing
/// was readable. A scan degraded only in its identity files says that, instead
/// of reporting an unreadable count of zero.
pub fn published_status(source_status: &str, snapshot: &source::ProcessSnapshot) -> String {
    published_status_from(source_status, &ScanReport::of(snapshot))
}

/// Everything about one scan that can reach the status line: counts and fixed
/// categories, never a record's own text (PX-004 review round 7).
///
/// The status is built from exactly this, so the longest status this app can
/// publish is a property of these fields at their widest — which is what
/// [`crate::refresh`] reserves snapshot-frame space for before it chooses how
/// many rows fit. Extracting the report from a snapshot is therefore separate
/// from wording it, so that reserve can be measured against the real builder
/// with every count at `usize::MAX`, instead of against the largest count a test
/// happens to be able to construct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Whether this scan fell short at all. A complete scan publishes no clause.
    pub degraded: bool,
    /// Why the enumeration root itself could not be listed, when it could not.
    /// No record count is then published: "0 unreadable" beside an empty table
    /// would claim the opposite of the truth.
    pub root: Option<source::MissingReason>,
    /// Records this scan listed.
    pub listed: usize,
    /// Records that existed and could not be read.
    pub unreadable: usize,
    /// Entries this scan listed but never read, its own record bound reached.
    pub capped: usize,
    /// Whether a global identity file could not be read.
    pub identity_incomplete: bool,
}

impl ScanReport {
    /// What `snapshot` says about its own shortfalls.
    pub fn of(snapshot: &source::ProcessSnapshot) -> Self {
        let source::Completeness::Incomplete { skipped, issues } = &snapshot.completeness else {
            return Self::default();
        };
        Self {
            degraded: true,
            root: issues
                .iter()
                .find(|issue| issue.scope == source::IssueScope::Root)
                .map(|issue| issue.reason),
            listed: snapshot.records.len(),
            unreadable: skipped.count(),
            // Records omitted by the collector's own bound were never read:
            // publishing them as unreadable would claim a read failure or a
            // permission problem that never happened.
            capped: snapshot.capped.count(),
            identity_incomplete: issues.iter().any(|issue| {
                matches!(
                    issue.scope,
                    source::IssueScope::HostIdentity
                        | source::IssueScope::BootIdentity
                        | source::IssueScope::PidNamespace
                )
            }),
        }
    }
}

/// The published status of a scan that reported `scan`.
pub fn published_status_from(source_status: &str, scan: &ScanReport) -> String {
    let label = bounded(source_status, MAX_SOURCE_STATUS_BYTES);
    if !scan.degraded {
        return label;
    }
    let mut clauses = vec!["incomplete scan".to_string()];
    if let Some(root) = scan.root {
        clauses.push(format!("process list unavailable: {}", root.describe()));
    } else {
        let listed = scan.listed;
        clauses.push(format!("{listed} {} listed", plural(listed)));
        if scan.unreadable > 0 {
            clauses.push(format!("{} unreadable", scan.unreadable));
        }
    }
    if scan.capped > 0 {
        clauses.push(format!("{} beyond the record limit", scan.capped));
    }
    if scan.identity_incomplete {
        clauses.push("host identity incomplete".to_string());
    }
    format!("{label} · {}", clauses.join(" · "))
}

/// The most of a source's own status label the published status carries, in
/// encoded bytes (PX-004 review round 7).
///
/// A label is source-owned text of no fixed length, and the published status is
/// charged against the catch-up snapshot frame *before* the rows are, so an
/// unbounded label would be an unbounded charge against the row budget — the
/// same defect as charging a degraded scan's own clauses against it. Bounding
/// the label is what makes the longest status this app can publish a constant.
pub const MAX_SOURCE_STATUS_BYTES: usize = 256;

/// The widest label [`published_status`] can emit: [`MAX_SOURCE_STATUS_BYTES`]
/// plus the ellipsis that marks a label as cut.
pub const MAX_PUBLISHED_LABEL_BYTES: usize = MAX_SOURCE_STATUS_BYTES + ELLIPSIS.len_utf8();

/// Marks text this app had to cut. Never a character a source supplied.
const ELLIPSIS: char = '…';

/// `text`, cut to at most `limit` encoded bytes at a character boundary and
/// marked with [`ELLIPSIS`] when it was cut.
///
/// Cutting is marked rather than silent, and the mark is this app's own
/// character. The cut is by encoded bytes because the budget it protects is a
/// byte budget, and it lands on a character boundary because a `String` may not
/// hold half a code point.
pub(crate) fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut cut = String::with_capacity(end + ELLIPSIS.len_utf8());
    cut.push_str(&text[..end]);
    cut.push(ELLIPSIS);
    cut
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        "process"
    } else {
        "processes"
    }
}

/// Publishes the shell, its system summary, its sort controls and `items` in
/// one transaction.
///
/// `sort_enabled` enables the sort buttons, which a shell with a source does
/// and an uncollected one does not: sorting a table that samples nothing has
/// no meaning, and a disabled control is refused by event validation (§7.4)
/// rather than merely discouraged. No button is marked active: until the first
/// click the rows are in their source's order, as before sorting existed.
///
/// `items` must already be bounded to what one catch-up snapshot of the
/// resulting model can carry: this is the start path, and a client that
/// attaches to this session at any later moment is brought up by a single
/// snapshot transaction of exactly this model (§18, §26). The bound is applied
/// where the rows are chosen, by `refresh::ProcessView::start`, because only
/// there can the status honestly say how many rows were left out.
pub(crate) fn initialize_rows(
    session: &Session,
    items: Vec<srui_semantic_tree::ModelItem>,
    status: &str,
    summary: &summary::Properties,
    sort_enabled: bool,
) -> Result<(), SessionError> {
    // §26 bounds the items in a single model mutation batch, and a live host can
    // hold more processes than that bound. The rows are therefore published as
    // consecutive bounded batches inside the one transaction that creates the
    // shell, so a large snapshot still arrives atomically instead of aborting
    // initialization.
    //
    // The batch must satisfy both bounds: this store's own limit, read before the
    // transaction opens because the session lock is held for the closure's
    // duration, and the wire default every conforming decoder enforces. A
    // session minted with a raised local limit would otherwise emit a
    // MODEL_INSERT that every client rejects at decode.
    let batch = session
        .with_store(|store| store.limits().max_items_per_model_operation)
        .clamp(1, srui_semantic_tree::DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION);
    session.transaction(|ui| {
        ui.apply_op(&Operation::create_model(MODEL, TypeRef::TABLE, 0))?;
        for (batch_index, chunk) in items.chunks(batch).enumerate() {
            ui.apply_op(&Operation::model_insert(
                MODEL,
                (batch_index * batch) as u64,
                chunk.to_vec(),
            ))?;
        }
        Surface::builder(SURFACE).label(TITLE).create(ui)?;
        Column::builder(COLUMN)
            .parent(SURFACE)
            .spacing_role(SpacingRole::Normal)
            .padding_role(PaddingRole::Normal)
            .grow(1.0)
            .create(ui)?;
        Text::builder(HEADING)
            .parent(COLUMN)
            .text(TITLE)
            .role(TextRole::Heading)
            .create(ui)?;
        Text::builder(STATUS)
            .parent(COLUMN)
            .text(status)
            .role(TextRole::Status)
            .create(ui)?;
        ui.set(STATUS, READ_ONLY, true)?;
        // The system summary, between the status line and the filter controls:
        // standard Text and Progress nodes only (PX-007).
        for operation in summary::create_operations(summary) {
            ui.apply_op(&operation)?;
        }
        // The filter controls: standard TextInput and Text caption in a Row,
        // between the system summary and the sort controls (PX-010).
        Row::builder(FILTER_ROW)
            .parent(COLUMN)
            .spacing_role(SpacingRole::Normal)
            .create(ui)?;
        TextInput::builder(FILTER_INPUT)
            .parent(FILTER_ROW)
            .label(FILTER_LABEL)
            .placeholder(FILTER_PLACEHOLDER)
            .value("")
            .role(InputRole::Search)
            .accessible_description(FILTER_DESCRIPTION)
            .create(ui)?;
        ui.set(FILTER_INPUT, ENABLED, sort_enabled)?;
        let initial_filter_status = if sort_enabled {
            let shown = items.len();
            if shown == 0 {
                "Filter: none · 0 rows".to_string()
            } else {
                format!("Filter: none · showing all {shown} rows")
            }
        } else {
            UNCOLLECTED_FILTER_STATUS.to_string()
        };
        Text::builder(FILTER_STATUS)
            .parent(FILTER_ROW)
            .text(initial_filter_status)
            .role(if sort_enabled {
                TextRole::Status
            } else {
                TextRole::Caption
            })
            .create(ui)?;
        // The sort controls: standard Buttons in a Row, one per catalogued
        // column, whose labels state the active order and whose activation is
        // the only client input this app handles (PX-009).
        Row::builder(SORT_ROW)
            .parent(COLUMN)
            .spacing_role(SpacingRole::Normal)
            .create(ui)?;
        for (button, field) in SORT_BUTTONS {
            Button::builder(button)
                .parent(SORT_ROW)
                .label(sort::sort_button_label(field, None))
                .create(ui)?;
            ui.set(button, ENABLED, sort_enabled)?;
        }
        // The column chooser: standard Buttons in a Row, whose labels state
        // what a click does to the default layout the table starts with
        // (PX-068). Like the sort buttons, they are disabled while nothing is
        // collected.
        let layout = catalogue::ColumnLayout::default();
        Row::builder(COLUMN_CHOOSER_ROW)
            .parent(COLUMN)
            .spacing_role(SpacingRole::Normal)
            .create(ui)?;
        for (button, action) in COLUMN_CHOOSER_BUTTONS {
            Button::builder(button)
                .parent(COLUMN_CHOOSER_ROW)
                .label(action.label(&layout))
                .create(ui)?;
            ui.set(button, ENABLED, sort_enabled && action.enabled(&layout))?;
        }
        Table::builder(TABLE)
            .parent(COLUMN)
            .model_ref(MODEL)
            // Headings come from the same catalogued column list the cells
            // are built from, so a column cannot be labeled as something other
            // than what the server published into it (PX-005, PX-042).
            .columns(refresh::column_headings(&layout))
            // A native selection, local presentation state (§22.5) the client
            // anchors to item IDs, so it follows its process when the rows are
            // reordered (PX-009). No handler acts on it: the server attaches no
            // meaning to a selection yet.
            .selection_mode(SelectionMode::Single)
            .label("Processes")
            .grow(1.0)
            .create(ui)?;
        ui.set(TABLE, READ_ONLY, true)?;
        Ok(())
    })?;
    Ok(())
}

/// Deterministic fixture mutation: scalar properties only, preserving every node ID.
pub fn update_title(session: &Session, title: &str) -> Result<(), SessionError> {
    session.transaction(|ui| {
        ui.set(SURFACE, LABEL, title)?;
        ui.set(HEADING, TEXT, title)?;
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use source::{
        Completeness, EnumerationIssue, IssueScope, MissingReason, ProcessSource, SkippedRecords,
    };

    /// The completeness of a scan that could not read `pids`, naming each one,
    /// exactly as a real scan records them.
    fn denied(pids: &[u32]) -> Completeness {
        let mut skipped = SkippedRecords::with_limit(pids.len());
        let mut issues = Vec::new();
        for pid in pids {
            skipped.record(*pid);
            issues.push(EnumerationIssue {
                scope: IssueScope::Process(*pid),
                reason: MissingReason::Denied,
                detail: "denied".into(),
            });
        }
        Completeness::from_scan(skipped, issues)
    }

    fn empty_snapshot(completeness: Completeness) -> source::ProcessSnapshot {
        let mut snapshot = source::FakeProcessSource.snapshot();
        snapshot.records.clear();
        snapshot.completeness = completeness;
        snapshot
    }

    #[test]
    fn authoritative_empty_and_incomplete_scans_are_distinct_states() {
        let label = source::FAKE_STATUS_TEXT;
        // A complete scan is labeled exactly as the source describes itself.
        assert_eq!(
            published_status(label, &empty_snapshot(Completeness::Complete)),
            label
        );
        assert_eq!(
            published_status(label, &source::FakeProcessSource.snapshot()),
            label
        );
        let degraded = denied(&[7, 8, 9, 10]);
        // An empty degraded scan never reads like an authoritative empty result.
        let empty_but_degraded = published_status(label, &empty_snapshot(degraded.clone()));
        assert_eq!(
            empty_but_degraded,
            format!("{label} · incomplete scan · 0 processes listed · 4 unreadable")
        );
        assert_ne!(
            empty_but_degraded,
            published_status(label, &empty_snapshot(Completeness::Complete))
        );
        let mut partial = source::FakeProcessSource.snapshot();
        partial.completeness = degraded;
        assert_eq!(
            published_status(label, &partial),
            format!("{label} · incomplete scan · 3 processes listed · 4 unreadable")
        );
        let mut single = empty_snapshot(denied(&[7]));
        single.records = source::FakeProcessSource.snapshot().records;
        single.records.truncate(1);
        assert_eq!(
            published_status(label, &single),
            format!("{label} · incomplete scan · 1 process listed · 1 unreadable")
        );
    }

    #[test]
    fn a_scan_that_read_nothing_never_publishes_a_count_of_zero_unreadable() {
        let label = source::FAKE_STATUS_TEXT;
        // The root itself could not be listed: no record was even reached, so
        // `skipped` is legitimately 0 and "0 unreadable" would read as "every
        // process was readable" beside an empty table.
        let unlistable = empty_snapshot(Completeness::from_scan(
            SkippedRecords::unenumerable(),
            vec![EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Unavailable,
                detail: "/proc: No such file or directory (os error 2)".into(),
            }],
        ));
        let published = published_status(label, &unlistable);
        assert_eq!(
            published,
            format!("{label} · incomplete scan · process list unavailable: unavailable")
        );
        assert!(!published.contains("unreadable"), "{published}");
        let denied_root = empty_snapshot(Completeness::from_scan(
            SkippedRecords::unenumerable(),
            vec![EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Denied,
                detail: "/proc: Permission denied (os error 13)".into(),
            }],
        ));
        assert_eq!(
            published_status(label, &denied_root),
            format!("{label} · incomplete scan · process list unavailable: permission denied")
        );
        // Only the identity files were unreadable: every listed record is real,
        // and nothing was skipped, so no unreadable count is published either.
        let mut identity_only = source::FakeProcessSource.snapshot();
        identity_only.completeness = Completeness::from_scan(
            SkippedRecords::none(),
            vec![EnumerationIssue {
                scope: IssueScope::BootIdentity,
                reason: MissingReason::Unavailable,
                detail: "sys/kernel/random/boot_id: unavailable".into(),
            }],
        );
        let published = published_status(label, &identity_only);
        assert_eq!(
            published,
            format!("{label} · incomplete scan · 3 processes listed · host identity incomplete")
        );
        assert!(!published.contains("unreadable"), "{published}");
    }

    #[test]
    fn empty_shell_has_exact_structure_and_no_action_or_terminal_nodes() {
        let session = Session::mint();
        initialize(&session).unwrap();
        assert_eq!(session.current_revision(), 1);
        session.with_store(|store| {
            // The five shell nodes, the fourteen of the system summary
            // (PX-007), the three of the filter bar (PX-010), the sort row
            // with its four buttons (PX-009) and the column chooser's row with
            // its thirteen buttons (PX-068).
            assert_eq!(store.node_count(), 41);
            assert_eq!(SHELL_NODE_COUNT, 41);
            assert_eq!(store.root_ids(), &[SURFACE]);
            assert_eq!(store.children_of(SURFACE), Some([COLUMN].as_slice()));
            assert_eq!(
                store.children_of(COLUMN),
                Some(
                    [
                        HEADING,
                        STATUS,
                        summary::SUMMARY,
                        FILTER_ROW,
                        SORT_ROW,
                        COLUMN_CHOOSER_ROW,
                        TABLE
                    ]
                    .as_slice()
                )
            );
            assert_eq!(
                store.children_of(FILTER_ROW),
                Some([FILTER_INPUT, FILTER_STATUS].as_slice())
            );
            assert_eq!(
                store.children_of(SORT_ROW),
                Some([SORT_PID, SORT_NAME, SORT_RESIDENT, SORT_CPU].as_slice())
            );
            assert_eq!(
                store.children_of(COLUMN_CHOOSER_ROW),
                Some(COLUMN_CHOOSER_BUTTONS.map(|(button, _)| button).as_slice())
            );
            assert_eq!(
                store.children_of(summary::SUMMARY),
                Some(
                    [
                        summary::CPU_ROW,
                        summary::MEMORY_ROW,
                        summary::SWAP_ROW,
                        summary::LOAD_TEXT,
                        summary::UPTIME_TEXT,
                        summary::PROCESSES_TEXT,
                        summary::FRESHNESS_TEXT,
                    ]
                    .as_slice()
                )
            );
            // Each bar precedes its line in its row.
            for (row, bar, line) in [
                (summary::CPU_ROW, summary::CPU_BAR, summary::CPU_TEXT),
                (
                    summary::MEMORY_ROW,
                    summary::MEMORY_BAR,
                    summary::MEMORY_TEXT,
                ),
                (summary::SWAP_ROW, summary::SWAP_BAR, summary::SWAP_TEXT),
            ] {
                assert_eq!(store.children_of(row), Some([bar, line].as_slice()));
            }
            let shell = [
                (SURFACE, TypeRef::SURFACE),
                (COLUMN, TypeRef::COLUMN),
                (HEADING, TypeRef::TEXT),
                (STATUS, TypeRef::TEXT),
                (FILTER_ROW, TypeRef::ROW),
                (FILTER_INPUT, TypeRef::TEXT_INPUT),
                (FILTER_STATUS, TypeRef::TEXT),
                (SORT_ROW, TypeRef::ROW),
                (SORT_PID, TypeRef::BUTTON),
                (SORT_NAME, TypeRef::BUTTON),
                (SORT_RESIDENT, TypeRef::BUTTON),
                (SORT_CPU, TypeRef::BUTTON),
                (COLUMN_CHOOSER_ROW, TypeRef::ROW),
                (TABLE, TypeRef::TABLE),
            ];
            let summary_nodes = summary::LAYOUT.map(|(id, node_type, _)| (id, node_type));
            let chooser = COLUMN_CHOOSER_BUTTONS.map(|(id, _)| (id, TypeRef::BUTTON));
            for (id, expected_type) in shell.into_iter().chain(summary_nodes).chain(chooser) {
                let node = store.get_node(id).unwrap();
                assert_eq!(node.node_type, expected_type);
                assert!(!node.has_property(ACTIONS));
                assert!(!node.has_property(ACTION_KEY));
            }
            // The filter controls of a shell that samples nothing are disabled:
            let filter_input = store.get_node(FILTER_INPUT).unwrap();
            assert_eq!(
                filter_input.get_property(LABEL),
                Some(&Value::String(FILTER_LABEL.into()))
            );
            assert_eq!(
                filter_input.get_property(PLACEHOLDER),
                Some(&Value::String(FILTER_PLACEHOLDER.into()))
            );
            assert_eq!(
                filter_input.get_property(ENABLED),
                Some(&Value::Bool(false))
            );
            let filter_status = store.get_node(FILTER_STATUS).unwrap();
            assert_eq!(
                filter_status.get_property(TEXT),
                Some(&Value::String(UNCOLLECTED_FILTER_STATUS.into()))
            );
            // The sort buttons of a shell that samples nothing are disabled:
            // there is no collection for them to order, and a disabled control
            // is refused by event validation rather than merely discouraged.
            // No button is marked active: nothing is sorted yet (PX-009).
            for (button, label) in [
                (SORT_PID, "Sort by PID"),
                (SORT_NAME, "Sort by Name"),
                (SORT_RESIDENT, "Sort by Resident"),
                (SORT_CPU, "Sort by CPU"),
            ] {
                let node = store.get_node(button).unwrap();
                assert_eq!(node.get_property(LABEL), Some(&Value::String(label.into())));
                assert_eq!(node.get_property(ENABLED), Some(&Value::Bool(false)));
            }
            // So are the column chooser's, labelled for the default layout
            // (PX-068).
            for (button, label) in [
                (COLUMN_TOGGLE_PID, "Hide PID column"),
                (COLUMN_EARLIER_PID, "Move PID column left"),
                (COLUMN_LATER_PID, "Move PID column right"),
                (COLUMN_TOGGLE_NAME, "Hide Name column"),
                (COLUMN_EARLIER_NAME, "Move Name column left"),
                (COLUMN_LATER_NAME, "Move Name column right"),
                (COLUMN_TOGGLE_RESIDENT, "Hide Resident column"),
                (COLUMN_EARLIER_RESIDENT, "Move Resident column left"),
                (COLUMN_LATER_RESIDENT, "Move Resident column right"),
                (COLUMN_TOGGLE_CPU, "Hide CPU column"),
                (COLUMN_EARLIER_CPU, "Move CPU column left"),
                (COLUMN_LATER_CPU, "Move CPU column right"),
                (COLUMNS_RESTORE, "Restore default columns"),
            ] {
                let node = store.get_node(button).unwrap();
                assert_eq!(node.get_property(LABEL), Some(&Value::String(label.into())));
                assert_eq!(node.get_property(ENABLED), Some(&Value::Bool(false)));
            }
            // An empty shell has sampled nothing and says so in every line.
            assert_eq!(
                store
                    .get_node(summary::FRESHNESS_TEXT)
                    .unwrap()
                    .get_property(TEXT),
                Some(&Value::String(
                    "No sample: process collection not started".into()
                ))
            );
            assert_eq!(
                store
                    .get_node(summary::MEMORY_TEXT)
                    .unwrap()
                    .get_property(TEXT),
                Some(&Value::String("Memory: Not sampled".into()))
            );
            for bar in [summary::CPU_BAR, summary::MEMORY_BAR, summary::SWAP_BAR] {
                let node = store.get_node(bar).unwrap();
                assert_eq!(
                    node.get_property(VISIBILITY),
                    Some(&Value::from(EnumToken::from(Visibility::Hidden)))
                );
                assert!(!node.has_property(VALUE), "no sample, so no bar value");
            }
            assert_eq!(
                store.get_node(STATUS).unwrap().get_property(READ_ONLY),
                Some(&Value::Bool(true))
            );
            assert_eq!(
                store.get_node(TABLE).unwrap().get_property(READ_ONLY),
                Some(&Value::Bool(true))
            );
            assert_eq!(store.get_node(TABLE).unwrap().model_ref(), Some(MODEL));
            assert_eq!(store.get_model(MODEL).unwrap().item_count, 0);
            assert_eq!(
                store.get_node(TABLE).unwrap().get_property(COLUMNS),
                Some(&Value::List(vec![
                    Value::String("PID".into()),
                    Value::String("Name".into()),
                    Value::String("Resident".into()),
                    Value::String("CPU (100% = 1 CPU)".into())
                ]))
            );
        });
    }

    #[test]
    fn chooser_controls_state_what_a_click_does_and_disable_what_it_cannot() {
        use crate::catalogue::{ColumnLayout, ProcessField::*};
        let default = ColumnLayout::default();
        let enabled: Vec<bool> = COLUMN_CHOOSER_BUTTONS
            .iter()
            .map(|(_, action)| action.enabled(&default))
            .collect();
        // Every column can be hidden; the first cannot move left, the last
        // cannot move right, and the defaults are already shown.
        assert_eq!(
            enabled,
            [true, false, true, true, true, true, true, true, true, true, true, false, false]
        );
        let hidden = ColumnAction::Toggle(Resident).apply(&default);
        assert_eq!(hidden.fields(), [Pid, Name, Cpu]);
        assert_eq!(
            ColumnAction::Toggle(Resident).label(&hidden),
            "Show Resident column"
        );
        assert!(ColumnAction::Toggle(Resident).enabled(&hidden));
        assert!(!ColumnAction::Earlier(Resident).enabled(&hidden));
        assert!(!ColumnAction::Later(Resident).enabled(&hidden));
        assert!(ColumnAction::RestoreDefaults.enabled(&hidden));
        assert_eq!(ColumnAction::RestoreDefaults.apply(&hidden), default);
        assert_eq!(
            ColumnAction::Toggle(Resident).apply(&hidden).fields(),
            [Pid, Name, Cpu, Resident]
        );
        // The last visible column cannot be hidden.
        let single = ColumnLayout::new(&[Cpu]).unwrap();
        assert!(!ColumnAction::Toggle(Cpu).enabled(&single));
        assert_eq!(ColumnAction::Toggle(Cpu).apply(&single), single);
        // No chooser label reads as a sort control.
        for (_, action) in COLUMN_CHOOSER_BUTTONS {
            assert!(!action.label(&default).to_lowercase().contains("sort"));
        }
    }

    #[test]
    fn title_fixture_preserves_nodes_and_empty_model() {
        let session = Session::mint();
        initialize(&session).unwrap();
        update_title(&session, FIXTURE_TITLE).unwrap();
        assert_eq!(session.current_revision(), 2);
        session.with_store(|store| {
            assert_eq!(store.node_count(), SHELL_NODE_COUNT);
            assert_eq!(
                store.get_node(SURFACE).unwrap().get_property(LABEL),
                Some(&Value::String(FIXTURE_TITLE.into()))
            );
            assert_eq!(
                store.get_node(HEADING).unwrap().get_property(TEXT),
                Some(&Value::String(FIXTURE_TITLE.into()))
            );
            assert_eq!(
                store.children_of(COLUMN),
                Some(
                    [
                        HEADING,
                        STATUS,
                        summary::SUMMARY,
                        FILTER_ROW,
                        SORT_ROW,
                        COLUMN_CHOOSER_ROW,
                        TABLE
                    ]
                    .as_slice()
                )
            );
            assert_eq!(store.get_model(MODEL).unwrap().item_count, 0);
        });
    }

    #[test]
    fn fixture_before_initialization_is_rejected_without_partial_state() {
        let session = Session::mint();
        assert!(update_title(&session, FIXTURE_TITLE).is_err());
        assert_eq!(session.current_revision(), 0);
        assert_eq!(session.node_count(), 0);
    }

    #[test]
    fn repeated_initialization_is_rejected_without_changing_existing_shell() {
        let session = Session::mint();
        initialize(&session).unwrap();
        update_title(&session, FIXTURE_TITLE).unwrap();
        assert!(initialize(&session).is_err());
        assert_eq!(session.current_revision(), 2);
        session.with_store(|store| {
            assert_eq!(
                store.get_node(SURFACE).unwrap().get_property(LABEL),
                Some(&Value::String(FIXTURE_TITLE.into()))
            );
            assert_eq!(store.node_count(), SHELL_NODE_COUNT);
        });
    }
}
