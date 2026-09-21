//! Read-only Process Explorer shell using existing SRUI widgets and transactions
//! (design §§6–8, 12, 22, 29; PX-001/PX-002/PX-003). No action handlers are
//! installed and no process is ever opened for control.

pub mod procfs;
mod projection;
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

/// Samples only the injected source and publishes its model rows atomically with the shell,
/// labeling the status with the source's own truthful description rather than a fixed
/// fixture string. No periodic collection or process actions are installed.
pub fn initialize_from_source(
    session: &Session,
    source: &mut impl source::ProcessSource,
) -> Result<source::ProcessSnapshot, Box<dyn std::error::Error>> {
    let status = source.status_text().to_string();
    let snapshot = source.snapshot();
    let items = projection::SessionItemIds::default().project(&snapshot)?;
    initialize_rows(session, items, &published_status(&status, &snapshot))?;
    Ok(snapshot)
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
    let source::Completeness::Incomplete { skipped, issues } = &snapshot.completeness else {
        return source_status.to_string();
    };
    let mut clauses = vec!["incomplete scan".to_string()];
    let root = issues
        .iter()
        .find(|issue| issue.scope == source::IssueScope::Root);
    if let Some(root) = root {
        clauses.push(format!(
            "process list unavailable: {}",
            root.reason.describe()
        ));
    } else {
        let listed = snapshot.records.len();
        clauses.push(format!("{listed} {} listed", plural(listed)));
        if *skipped > 0 {
            clauses.push(format!("{skipped} unreadable"));
        }
    }
    // Records omitted by the collector's own bound were never read: publishing
    // them as unreadable would claim a read failure or a permission problem that
    // never happened.
    if snapshot.capped > 0 {
        clauses.push(format!("{} beyond the record limit", snapshot.capped));
    }
    if issues.iter().any(|issue| {
        matches!(
            issue.scope,
            source::IssueScope::HostIdentity
                | source::IssueScope::BootIdentity
                | source::IssueScope::PidNamespace
        )
    }) {
        clauses.push("host identity incomplete".to_string());
    }
    format!("{source_status} · {}", clauses.join(" · "))
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        "process"
    } else {
        "processes"
    }
}

fn initialize_rows(
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
    use source::{Completeness, EnumerationIssue, IssueScope, MissingReason, ProcessSource};

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
        let degraded = Completeness::from_scan(
            4,
            vec![EnumerationIssue {
                scope: IssueScope::Process(7),
                reason: MissingReason::Denied,
                detail: "denied".into(),
            }],
        );
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
        let mut single = empty_snapshot(Completeness::from_scan(1, vec![]));
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
            0,
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
            0,
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
            0,
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
