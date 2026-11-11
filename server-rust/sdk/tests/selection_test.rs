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

// -----------------------------------------------------------------------------------------------
// Round 1: every authoritative way to lose the promoted item, and raw promotions (§6.3, §8).
// -----------------------------------------------------------------------------------------------

fn row(id: u64) -> srui_semantic_tree::ModelItem {
    srui_semantic_tree::ModelItem::with_value(ItemId::new(id), Value::String(format!("row {id}")))
}

fn promoted_41() -> Session {
    let session = session(SelectionMode::Single);
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(41)))
        .unwrap();
    session
}

#[test]
fn reset_range_replacing_the_item_clears_the_promotion() {
    // The verifier's probe: ItemId 41 sits at index 40.
    let session = promoted_41();
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::model_reset_range(
                MODEL,
                40,
                [row(50_000)],
                None,
            ))
        })
        .unwrap();
    assert_eq!(promoted(&session), None);
    session.with_store(|store| assert!(srui_sdk::selection::dangling_promotions(store).is_empty()));

    // A reset that keeps the item (re-sent at the same or a different index in the window)
    // keeps the promotion.
    let session = promoted_41();
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::model_reset_range(
                MODEL,
                40,
                [row(50_001), row(41)],
                None,
            ))
        })
        .unwrap();
    assert_eq!(promoted(&session), Some(ItemId::new(41)));
}

#[test]
fn positional_update_displacing_the_item_clears_the_promotion() {
    let session = promoted_41();
    session
        .transaction(|ui| ui.apply_op(&Operation::model_update(MODEL, Some(40), [row(60_000)])))
        .unwrap();
    assert_eq!(promoted(&session), None);
}

#[test]
fn model_ref_change_or_clear_clears_a_promotion_the_new_model_lacks() {
    let other = ModelId::new(8);
    let with_item = ModelId::new(9);
    for (target, keeps) in [(Some(other), false), (Some(with_item), true), (None, false)] {
        let session = promoted_41();
        session
            .transaction(|ui| {
                ui.apply_op(&Operation::create_model(other, TypeRef::TABLE, 0))?;
                ui.apply_op(&Operation::model_insert(other, 0, [row(1)]))?;
                ui.apply_op(&Operation::create_model(with_item, TypeRef::TABLE, 0))?;
                ui.apply_op(&Operation::model_insert(with_item, 0, [row(41)]))?;
                match target {
                    Some(model) => ui
                        .set(TABLE, MODEL_REF, Value::UnsignedInt(model.get()))
                        .map(|_| ()),
                    None => ui.clear(TABLE, MODEL_REF).map(|_| ()),
                }
            })
            .unwrap();
        assert_eq!(
            promoted(&session),
            keeps.then_some(ItemId::new(41)),
            "model_ref -> {target:?}"
        );
    }
}

#[test]
fn deleting_the_collection_node_removes_its_promotion() {
    let session = promoted_41();
    session
        .transaction(|ui| ui.delete(TABLE).map(|_| ()))
        .unwrap();
    session.with_store(|store| {
        assert!(store.get_node(NodeId::new(TABLE)).is_none());
        assert!(srui_sdk::selection::dangling_promotions(store).is_empty());
    });
}

#[test]
fn raw_set_of_an_unknown_item_is_refused_and_rolled_back() {
    let session = session(SelectionMode::Single);
    let revision = session.current_revision();
    let err = session
        .transaction(|ui| {
            ui.set(TABLE, LABEL, "renamed")?;
            ui.set(TABLE, VALUE, Value::ItemId(ItemId::new(999_999)))
                .map(|_| ())
        })
        .expect_err("unknown item");
    assert!(
        matches!(err, SdkError::Store(StoreError::ItemNotFound(id)) if id == ItemId::new(999_999)),
        "{err:?}"
    );
    assert_eq!(session.current_revision(), revision);
    assert_eq!(promoted(&session), None);
    session.with_store(|store| {
        assert!(!store
            .get_node(NodeId::new(TABLE))
            .unwrap()
            .has_property(LABEL));
    });

    // A raw promotion of a held item is accepted.
    session
        .transaction(|ui| {
            ui.set(TABLE, VALUE, Value::ItemId(ItemId::new(7)))
                .map(|_| ())
        })
        .unwrap();
    assert_eq!(promoted(&session), Some(ItemId::new(7)));
}

#[test]
fn raw_batch_and_create_node_of_an_unknown_item_are_refused() {
    let session = session(SelectionMode::Single);
    let err = session
        .transaction(|ui| {
            ui.apply_op(&Operation::batch_property_set(
                NodeId::new(TABLE),
                [(VALUE, Value::ItemId(ItemId::new(999_999)))],
            ))
        })
        .expect_err("unknown item in batch");
    assert!(
        matches!(err, SdkError::Store(StoreError::ItemNotFound(_))),
        "{err:?}"
    );

    let err = session
        .transaction(|ui| {
            ui.create_node(
                4u64,
                TypeRef::TABLE,
                Some(NodeId::new(SURFACE)),
                None,
                [
                    (MODEL_REF, Value::UnsignedInt(MODEL.get())),
                    (
                        SELECTION_MODE,
                        Value::EnumToken(SelectionMode::Single.into()),
                    ),
                    (VALUE, Value::ItemId(ItemId::new(999_999))),
                ],
            )
        })
        .expect_err("unknown item in create_node");
    assert!(
        matches!(err, SdkError::Store(StoreError::ItemNotFound(_))),
        "{err:?}"
    );
    session.with_store(|store| assert!(store.get_node(NodeId::new(4)).is_none()));
}

#[test]
fn promoting_then_deleting_the_item_in_one_transaction_is_refused() {
    let session = session(SelectionMode::Single);
    let err = session
        .transaction(|ui| {
            ui.promote_selection(TABLE, ItemId::new(9))?;
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(9)]))
        })
        .expect_err("a promotion assigned in this transaction must name a held item");
    assert!(matches!(err, SdkError::Store(StoreError::ItemNotFound(_))));
}

#[test]
fn transaction_custom_settles_promotions_too() {
    // Removal is cleared in the same commit.
    let session = promoted_41();
    session
        .transaction_custom(|ui| {
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(41)]))
                .map_err(|e| e.to_string())
        })
        .expect("custom commit");
    assert_eq!(promoted(&session), None);

    // An unknown raw promotion is refused and rolled back.
    let session = self::session(SelectionMode::Single);
    let revision = session.current_revision();
    let err = session
        .transaction_custom(|ui| {
            ui.set(TABLE, VALUE, Value::ItemId(ItemId::new(999_999)))
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .expect_err("unknown item");
    assert!(
        matches!(err, SdkError::Store(StoreError::ItemNotFound(_))),
        "{err:?}"
    );
    assert_eq!(session.current_revision(), revision);
    assert_eq!(promoted(&session), None);
}

// -----------------------------------------------------------------------------------------------
// PR review: a refused settlement swallowed by the callback must still refuse the commit.
// -----------------------------------------------------------------------------------------------

/// Promotes an unknown item, calls settlement, swallows its refusal and reports success.
fn promote_unknown_and_swallow_refusal(ui: &mut UiTransaction) -> Result<(), StoreError> {
    ui.set(TABLE, LABEL, "renamed")?;
    ui.set(TABLE, VALUE, Value::ItemId(ItemId::new(999_999)))?;
    assert!(matches!(
        ui.settle_promoted_selections(),
        Err(StoreError::ItemNotFound(_))
    ));
    Ok(())
}

fn assert_refused_and_rolled_back(session: &Session, revision: Revision, err: SdkError) {
    assert!(
        matches!(err, SdkError::Store(StoreError::ItemNotFound(id)) if id == ItemId::new(999_999)),
        "{err:?}"
    );
    assert_eq!(session.current_revision(), revision);
    assert_eq!(promoted(session), None);
    session.with_store(|store| {
        assert!(!store
            .get_node(NodeId::new(TABLE))
            .unwrap()
            .has_property(LABEL));
    });
}

#[test]
fn swallowed_settlement_refusal_is_refused_again_by_transaction() {
    let session = session(SelectionMode::Single);
    let revision = session.current_revision();
    let err = session
        .transaction(promote_unknown_and_swallow_refusal)
        .expect_err("the commit path settles again");
    assert_refused_and_rolled_back(&session, revision, err);
}

#[test]
fn swallowed_settlement_refusal_is_refused_again_by_transaction_custom() {
    let session = session(SelectionMode::Single);
    let revision = session.current_revision();
    let err = session
        .transaction_custom(|ui| promote_unknown_and_swallow_refusal(ui).map_err(|e| e.to_string()))
        .expect_err("the commit path settles again");
    assert_refused_and_rolled_back(&session, revision, err);
}

#[test]
fn a_failed_clear_leaves_the_transaction_unsettled() {
    // Op budget: the delete fits, the settlement's CLEAR_PROPERTY does not.
    let session = session(SelectionMode::Single);
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(3)))
        .unwrap();
    session.with_store(|store| {
        let mut ui = UiTransaction::new(store.clone_staging(), 1);
        ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(3)]))
            .unwrap();
        assert!(
            ui.settle_promoted_selections().is_err(),
            "the clear exceeds the op budget"
        );
        assert!(
            ui.settle_promoted_selections().is_err(),
            "still unsettled after a failed clear"
        );
    });
}
