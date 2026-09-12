//! Read-only Process Explorer shell using existing SRUI widgets and transactions
//! (design §§6–8, 12, 29; PX-001/PX-002). No action handlers are installed.

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

/// Samples only the injected source and publishes its model rows atomically with the shell.
/// No periodic collection or process actions are installed.
pub fn initialize_from_source(
    session: &Session,
    source: &mut impl source::ProcessSource,
) -> Result<source::ProcessSnapshot, Box<dyn std::error::Error>> {
    let snapshot = source.snapshot();
    let items = projection::SessionItemIds::default().project(&snapshot)?;
    initialize_rows(session, items, "Read-only · Fake process snapshot")?;
    Ok(snapshot)
}

fn initialize_rows(
    session: &Session,
    items: Vec<srui_semantic_tree::ModelItem>,
    status: &str,
) -> Result<(), SessionError> {
    session.transaction(|ui| {
        ui.apply_op(&Operation::create_model(MODEL, TypeRef::TABLE, 0))?;
        if !items.is_empty() {
            ui.apply_op(&Operation::model_insert(MODEL, 0, items.clone()))?;
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
