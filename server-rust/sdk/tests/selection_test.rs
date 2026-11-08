//! SDK server-promoted collection selection (§6.3, §7.6, §8, §12.1).

use srui_sdk::*;

const SURFACE: u64 = 1;
const TABLE: u64 = 2;
const MODEL: ModelId = ModelId::new(7);
const ROWS: u64 = 10_000;

fn session(mode: SelectionMode) -> Session {
    let session = Session::new("selection");
    session
        .transaction(|ui| {
            Surface::builder(SURFACE).label("Processes").create(ui)?;
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::TABLE, 0))?;
            ui.apply_op(&Operation::model_insert(
                MODEL,
                0,
                (0..ROWS).map(|i| {
                    srui_semantic_tree::ModelItem::with_value(
                        ItemId::new(i + 1),
                        Value::String(format!("row {i}")),
                    )
                }),
            ))?;
            Table::builder(TABLE)
                .parent(SURFACE)
                .model_ref(MODEL)
                .selection_mode(mode)
                .create(ui)?;
            Ok(())
        })
        .expect("setup");
    session
}

fn promoted(session: &Session) -> Option<ItemId> {
    session
        .with_store(|store| Table::from_store(store, NodeId::new(TABLE))?.promoted_selection(store))
}

#[test]
fn promotion_records_the_item_then_the_selection() {
    let session = session(SelectionMode::Single);
    session
        .with_store(|store| {
            let mut ui = UiTransaction::new(store.clone_staging(), 1_000);
            ui.promote_selection(TABLE, ItemId::new(9_001))?;
            let ops = ui.operations().to_vec();
            assert_eq!(ops.len(), 2);
            match &ops[0] {
                Operation::ModelResetRange {
                    id,
                    start_index,
                    items,
                    total_count,
                } => {
                    assert_eq!(*id, MODEL);
                    assert_eq!(*start_index, 9_000);
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].item_id, ItemId::new(9_001));
                    assert_eq!(*total_count, Some(ROWS));
                }
                other => panic!("expected a one-item MODEL_RESET_RANGE, got {other:?}"),
            }
            assert_eq!(
                ops[1],
                Operation::set_property(
                    NodeId::new(TABLE),
                    VALUE,
                    Value::ItemId(ItemId::new(9_001))
                )
            );
            Ok::<_, StoreError>(())
        })
        .expect("staged promotion");

    let table = Table::new(NodeId::new(TABLE));
    session
        .transaction(|ui| table.promote_selection(ui, ItemId::new(9_001)))
        .expect("promotion");
    assert_eq!(promoted(&session), Some(ItemId::new(9_001)));
    session.with_store(|store| {
        assert_eq!(store.get_model(MODEL).unwrap().item_count(), ROWS);
    });
}

#[test]
fn unknown_item_fails_explicitly_without_row_index_fallback() {
    let session = session(SelectionMode::Single);
    let revision = session.current_revision();
    let err = session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(ROWS + 5)))
        .expect_err("unknown item");
    assert!(
        matches!(err, SdkError::Store(StoreError::ItemNotFound(id)) if id == ItemId::new(ROWS + 5)),
        "{err:?}"
    );
    assert_eq!(session.current_revision(), revision, "nothing committed");
    assert_eq!(promoted(&session), None);
}

#[test]
fn selection_mode_none_refuses_promotion() {
    let session = session(SelectionMode::None);
    let err = session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(1)))
        .expect_err("selection disabled");
    assert!(
        matches!(
            err,
            SdkError::Store(StoreError::InvalidCollectionSelection { .. })
        ),
        "{err:?}"
    );
    assert_eq!(promoted(&session), None);

    // A raw SET_PROPERTY is refused by the store rule as well.
    let raw = session
        .transaction(|ui| {
            ui.set(TABLE, VALUE, Value::ItemId(ItemId::new(1)))
                .map(|_| ())
        })
        .expect_err("raw promotion with selection disabled");
    assert!(matches!(
        raw,
        SdkError::Store(StoreError::InvalidCollectionSelection { .. })
    ));
}

#[test]
fn non_collection_and_model_less_nodes_refuse_promotion() {
    let session = Session::new("selection");
    session
        .transaction(|ui| {
            Button::builder(1).label("Go").create(ui)?;
            List::builder(2)
                .selection_mode(SelectionMode::Single)
                .create(ui)?;
            Ok(())
        })
        .unwrap();
    for node in [1u64, 2] {
        let err = session
            .transaction(|ui| ui.promote_selection(node, ItemId::new(1)))
            .expect_err("refused");
        assert!(matches!(
            err,
            SdkError::Store(StoreError::InvalidCollectionSelection { .. })
        ));
    }
    let err = session
        .transaction(|ui| ui.promote_selection(99u64, ItemId::new(1)))
        .expect_err("missing node");
    assert!(matches!(err, SdkError::Store(StoreError::NodeNotFound(_))));
}

#[test]
fn moving_the_item_under_sorting_keeps_the_promotion() {
    let session = session(SelectionMode::Single);
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(500)))
        .unwrap();
    // A re-sort expressed as delete + insert of the same ItemId in one transaction.
    session
        .transaction(|ui| {
            let moved = ui
                .get_model(MODEL)
                .unwrap()
                .get_item_by_id(ItemId::new(500))
                .cloned()
                .unwrap();
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(500)]))?;
            ui.apply_op(&Operation::model_insert(MODEL, 0, [moved]))?;
            Ok(())
        })
        .unwrap();
    assert_eq!(promoted(&session), Some(ItemId::new(500)));
    session.with_store(|store| {
        assert_eq!(
            store.get_model(MODEL).unwrap().index_of(ItemId::new(500)),
            Some(0)
        );
    });
}

#[test]
fn deleting_the_item_clears_the_promotion_in_the_same_commit() {
    // By identity.
    let session = session(SelectionMode::Single);
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(42)))
        .unwrap();
    session
        .transaction(|ui| ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(42)])))
        .unwrap();
    assert_eq!(promoted(&session), None);

    // By index range, and an unrelated delete leaves the promotion alone.
    let session = self::session(SelectionMode::Multiple);
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(42)))
        .unwrap();
    session
        .transaction(|ui| ui.apply_op(&Operation::model_delete_range(MODEL, 100, 10)))
        .unwrap();
    assert_eq!(promoted(&session), Some(ItemId::new(42)));
    session
        .transaction(|ui| ui.apply_op(&Operation::model_delete_range(MODEL, 40, 5)))
        .unwrap();
    assert_eq!(promoted(&session), None);
}

#[test]
fn deletion_settles_into_recorded_operations() {
    let session = session(SelectionMode::Single);
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(3)))
        .unwrap();
    session
        .with_store(|store| {
            let mut ui = UiTransaction::new(store.clone_staging(), 1_000);
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(3)]))?;
            ui.settle_promoted_selections()?;
            ui.settle_promoted_selections()?;
            assert_eq!(
                ui.operations().last(),
                Some(&Operation::clear_property(NodeId::new(TABLE), VALUE))
            );
            assert_eq!(ui.operations().len(), 2, "settling twice records one clear");
            Ok::<_, StoreError>(())
        })
        .unwrap();
}

#[test]
fn a_later_promotion_replaces_the_previous_one_and_clear_withdraws_it() {
    let session = session(SelectionMode::Single);
    let table = Table::new(NodeId::new(TABLE));
    session
        .transaction(|ui| table.promote_selection(ui, ItemId::new(1)))
        .unwrap();
    session
        .transaction(|ui| table.promote_selection(ui, ItemId::new(2)))
        .unwrap();
    assert_eq!(promoted(&session), Some(ItemId::new(2)));
    let previous = session
        .transaction(|ui| table.clear_promoted_selection(ui))
        .unwrap();
    assert_eq!(previous, Some(ItemId::new(2)));
    assert_eq!(promoted(&session), None);
    let none = session
        .transaction(|ui| table.clear_promoted_selection(ui))
        .unwrap();
    assert_eq!(none, None);
}
