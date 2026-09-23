//! Read-only Process Explorer shell using existing SRUI widgets and transactions
//! (design §§6–8, 12, 22, 29; PX-001/PX-002/PX-003/PX-004). No action handlers
//! are installed and no process is ever opened for control.

pub mod procfs;
mod projection;
pub mod refresh;
pub mod source;

use srui_sdk::*;
use srui_sessiond::{Session, SessionError};

pub const SURFACE: NodeId = NodeId::new(1);
pub const COLUMN: NodeId = NodeId::new(2);
pub const HEADING: NodeId = NodeId::new(3);
pub const STATUS: NodeId = NodeId::new(4);
pub const TABLE: NodeId = NodeId::new(5);
pub const MODEL: ModelId = ModelId::new(1);
pub const TITLE: &str = "Process Explorer";
pub const FIXTURE_TITLE: &str = "Process Explorer — title fixture";
pub const STATUS_TEXT: &str = "Read-only · Process collection not started";

/// Creates the complete empty shell in one authoritative commit.
pub fn initialize(session: &Session) -> Result<(), SessionError> {
    initialize_rows(session, vec![], STATUS_TEXT)
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

/// Publishes the shell and `items` in one transaction.
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
        Table::builder(TABLE)
            .parent(COLUMN)
            .model_ref(MODEL)
            .columns([Value::String("PID".into()), Value::String("Name".into())])
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
            assert_eq!(store.node_count(), 5);
            assert_eq!(store.root_ids(), &[SURFACE]);
            assert_eq!(store.children_of(SURFACE), Some([COLUMN].as_slice()));
            assert_eq!(
                store.children_of(COLUMN),
                Some([HEADING, STATUS, TABLE].as_slice())
            );
            for (id, expected_type) in [
                (SURFACE, TypeRef::SURFACE),
                (COLUMN, TypeRef::COLUMN),
                (HEADING, TypeRef::TEXT),
                (STATUS, TypeRef::TEXT),
                (TABLE, TypeRef::TABLE),
            ] {
                let node = store.get_node(id).unwrap();
                assert_eq!(node.node_type, expected_type);
                assert!(!node.has_property(ACTIONS));
                assert!(!node.has_property(ACTION_KEY));
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
                    Value::String("Name".into())
                ]))
            );
        });
    }

    #[test]
    fn title_fixture_preserves_nodes_and_empty_model() {
        let session = Session::mint();
        initialize(&session).unwrap();
        update_title(&session, FIXTURE_TITLE).unwrap();
        assert_eq!(session.current_revision(), 2);
        session.with_store(|store| {
            assert_eq!(store.node_count(), 5);
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
                Some([HEADING, STATUS, TABLE].as_slice())
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
            assert_eq!(store.node_count(), 5);
        });
    }
}
