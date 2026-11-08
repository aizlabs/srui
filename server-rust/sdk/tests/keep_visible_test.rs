//! SDK and authoritative settlement of the server-hinted keep-in-view item, the standard
//! `keep_visible` property (§6.3, §8, §12.1). The hint follows the promoted-selection settlement rules,
//! independently of `value`.

use srui_sdk::*;

const SURFACE: u64 = 1;
const TABLE: u64 = 2;
const MODEL: ModelId = ModelId::new(7);
const ROWS: u64 = 10_000;

fn row(id: u64) -> srui_semantic_tree::ModelItem {
    srui_semantic_tree::ModelItem::with_value(ItemId::new(id), Value::String(format!("row {id}")))
}

fn session() -> Session {
    let session = Session::new("keep-visible");
    session
        .transaction(|ui| {
            Surface::builder(SURFACE).label("Processes").create(ui)?;
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::TABLE, 0))?;
            ui.apply_op(&Operation::model_insert(MODEL, 0, (1..=ROWS).map(row)))?;
            Table::builder(TABLE)
                .parent(SURFACE)
                .model_ref(MODEL)
                .selection_mode(SelectionMode::Single)
                .create(ui)?;
            Ok(())
        })
        .expect("setup");
    session
}

fn hinted(session: &Session) -> Option<ItemId> {
    session.with_store(|store| Table::from_store(store, NodeId::new(TABLE))?.keep_visible(store))
}

fn promoted(session: &Session) -> Option<ItemId> {
    session
        .with_store(|store| Table::from_store(store, NodeId::new(TABLE))?.promoted_selection(store))
}

fn hinted_41() -> Session {
    let session = session();
    session
        .transaction(|ui| ui.set_keep_visible(TABLE, ItemId::new(41)))
        .unwrap();
    session
}

fn assert_item_not_found(err: &SdkError, item: u64) {
    assert!(
        matches!(err, SdkError::Store(StoreError::ItemNotFound(id)) if *id == ItemId::new(item)),
        "{err:?}"
    );
}

#[test]
fn set_records_the_item_then_the_hint_and_never_touches_selection() {
    let session = session();
    session
        .with_store(|store| {
            let mut ui = UiTransaction::new(store.clone_staging(), 1_000);
            ui.set_keep_visible(TABLE, ItemId::new(9_001))?;
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
                    KEEP_VISIBLE,
                    Value::ItemId(ItemId::new(9_001))
                )
            );
            Ok::<_, StoreError>(())
        })
        .expect("staged hint");

    let table = Table::new(NodeId::new(TABLE));
    session
        .transaction(|ui| table.promote_selection(ui, ItemId::new(5)))
        .unwrap();
    session
        .transaction(|ui| table.set_keep_visible(ui, ItemId::new(9_001)))
        .expect("hint");
    assert_eq!(hinted(&session), Some(ItemId::new(9_001)));
    assert_eq!(promoted(&session), Some(ItemId::new(5)), "value untouched");
    session.with_store(|store| assert_eq!(store.get_model(MODEL).unwrap().item_count(), ROWS));

    let previous = session
        .transaction(|ui| table.clear_keep_visible(ui))
        .unwrap();
    assert_eq!(previous, Some(ItemId::new(9_001)));
    assert_eq!(hinted(&session), None);
    assert_eq!(promoted(&session), Some(ItemId::new(5)), "value untouched");
    let none = session
        .transaction(|ui| table.clear_keep_visible(ui))
        .unwrap();
    assert_eq!(none, None);
}

#[test]
fn unknown_item_fails_explicitly_without_row_index_fallback() {
    let session = session();
    let revision = session.current_revision();
    let err = session
        .transaction(|ui| ui.set_keep_visible(TABLE, ItemId::new(ROWS + 5)))
        .expect_err("unknown item");
    assert_item_not_found(&err, ROWS + 5);
    assert_eq!(session.current_revision(), revision, "nothing committed");
    assert_eq!(hinted(&session), None);
}

#[test]
fn non_collection_model_less_and_missing_nodes_refuse_it() {
    let session = Session::new("keep-visible");
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
            .transaction(|ui| ui.set_keep_visible(node, ItemId::new(1)))
            .expect_err("refused");
        assert!(
            matches!(err, SdkError::Store(StoreError::InvalidKeepVisible { .. })),
            "{err:?}"
        );
        // A raw SET_PROPERTY is refused by the store rule as well.
        let raw = session
            .transaction(|ui| {
                ui.set(node, KEEP_VISIBLE, Value::ItemId(ItemId::new(1)))
                    .map(|_| ())
            })
            .expect_err("raw hint");
        assert!(matches!(
            raw,
            SdkError::Store(StoreError::InvalidKeepVisible { .. })
        ));
    }
    let err = session
        .transaction(|ui| ui.set_keep_visible(99u64, ItemId::new(1)))
        .expect_err("missing node");
    assert!(matches!(err, SdkError::Store(StoreError::NodeNotFound(_))));
}

#[test]
fn moving_the_item_under_sorting_keeps_the_hint() {
    let session = hinted_41();
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(41)]))?;
            ui.apply_op(&Operation::model_insert(MODEL, 0, [row(41)]))
        })
        .unwrap();
    assert_eq!(hinted(&session), Some(ItemId::new(41)));
    session.with_store(|store| {
        assert_eq!(
            store.get_model(MODEL).unwrap().index_of(ItemId::new(41)),
            Some(0)
        );
    });
}

#[test]
fn every_removal_path_clears_the_hint_in_the_same_commit() {
    type Removal = fn(&mut UiTransaction) -> Result<(), StoreError>;
    let removals: [(&str, Removal); 5] = [
        ("delete by id", |ui| {
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(41)]))
        }),
        ("delete by range", |ui| {
            ui.apply_op(&Operation::model_delete_range(MODEL, 38, 5))
        }),
        ("reset range replacing it", |ui| {
            ui.apply_op(&Operation::model_reset_range(
                MODEL,
                40,
                [row(50_000)],
                None,
            ))
        }),
        ("positional update", |ui| {
            ui.apply_op(&Operation::model_update(MODEL, Some(40), [row(60_000)]))
        }),
        ("model_ref change", |ui| {
            let other = ModelId::new(8);
            ui.apply_op(&Operation::create_model(other, TypeRef::TABLE, 0))?;
            ui.apply_op(&Operation::model_insert(other, 0, [row(1)]))?;
            ui.set(TABLE, MODEL_REF, Value::UnsignedInt(other.get()))
                .map(|_| ())
        }),
    ];
    for (name, removal) in removals {
        let session = hinted_41();
        session
            .transaction(removal)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(hinted(&session), None, "{name}");
        session.with_store(|store| {
            assert!(
                srui_sdk::selection::dangling_keep_visible(store).is_empty(),
                "{name}"
            );
        });
    }

    // The same removal paths, recorded as one CLEAR_PROPERTY in the commit.
    let session = hinted_41();
    session
        .with_store(|store| {
            let mut ui = UiTransaction::new(store.clone_staging(), 1_000);
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(41)]))?;
            ui.settle_promoted_selections()?;
            ui.settle_promoted_selections()?;
            assert_eq!(
                ui.operations().last(),
                Some(&Operation::clear_property(NodeId::new(TABLE), KEEP_VISIBLE))
            );
            assert_eq!(ui.operations().len(), 2, "settling twice records one clear");
            Ok::<_, StoreError>(())
        })
        .unwrap();
}

#[test]
fn unrelated_changes_and_a_model_ref_to_a_model_with_the_item_keep_the_hint() {
    let session = hinted_41();
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::model_delete_range(MODEL, 100, 10))?;
            ui.apply_op(&Operation::model_reset_range(
                MODEL,
                40,
                [row(50_001), row(41)],
                None,
            ))?;
            let with_item = ModelId::new(9);
            ui.apply_op(&Operation::create_model(with_item, TypeRef::TABLE, 0))?;
            ui.apply_op(&Operation::model_insert(with_item, 0, [row(41)]))?;
            ui.set(TABLE, MODEL_REF, Value::UnsignedInt(with_item.get()))
                .map(|_| ())
        })
        .unwrap();
    assert_eq!(hinted(&session), Some(ItemId::new(41)));
}

#[test]
fn settling_a_hint_never_touches_the_promotion_and_vice_versa() {
    // Promotion and hint name different items; deleting the hinted one clears only the hint.
    let session = hinted_41();
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(7)))
        .unwrap();
    session
        .transaction(|ui| ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(41)])))
        .unwrap();
    assert_eq!(hinted(&session), None);
    assert_eq!(promoted(&session), Some(ItemId::new(7)));

    // And deleting the promoted one clears only the promotion.
    let session = hinted_41();
    session
        .transaction(|ui| ui.promote_selection(TABLE, ItemId::new(7)))
        .unwrap();
    session
        .transaction(|ui| ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(7)])))
        .unwrap();
    assert_eq!(hinted(&session), Some(ItemId::new(41)));
    assert_eq!(promoted(&session), None);
}

#[test]
fn raw_set_batch_and_create_of_an_unknown_item_are_refused_and_rolled_back() {
    let session = session();
    let revision = session.current_revision();
    let err = session
        .transaction(|ui| {
            ui.set(TABLE, LABEL, "renamed")?;
            ui.set(TABLE, KEEP_VISIBLE, Value::ItemId(ItemId::new(999_999)))
                .map(|_| ())
        })
        .expect_err("unknown item");
    assert_item_not_found(&err, 999_999);
    assert_eq!(session.current_revision(), revision);
    assert_eq!(hinted(&session), None);
    session.with_store(|store| {
        assert!(!store
            .get_node(NodeId::new(TABLE))
            .unwrap()
            .has_property(LABEL));
    });

    let err = session
        .transaction(|ui| {
            ui.apply_op(&Operation::batch_property_set(
                NodeId::new(TABLE),
                [(KEEP_VISIBLE, Value::ItemId(ItemId::new(999_998)))],
            ))
        })
        .expect_err("unknown item in batch");
    assert_item_not_found(&err, 999_998);

    let err = session
        .transaction(|ui| {
            ui.create_node(
                4u64,
                TypeRef::TABLE,
                Some(NodeId::new(SURFACE)),
                None,
                [
                    (MODEL_REF, Value::UnsignedInt(MODEL.get())),
                    (KEEP_VISIBLE, Value::ItemId(ItemId::new(999_997))),
                ],
            )
        })
        .expect_err("unknown item in create_node");
    assert_item_not_found(&err, 999_997);
    session.with_store(|store| assert!(store.get_node(NodeId::new(4)).is_none()));
    assert_eq!(session.current_revision(), revision);

    // A raw hint naming a held item is accepted.
    session
        .transaction(|ui| {
            ui.set(TABLE, KEEP_VISIBLE, Value::ItemId(ItemId::new(7)))
                .map(|_| ())
        })
        .unwrap();
    assert_eq!(hinted(&session), Some(ItemId::new(7)));
}

#[test]
fn hinting_then_deleting_the_item_in_one_transaction_is_refused() {
    let session = session();
    let err = session
        .transaction(|ui| {
            ui.set_keep_visible(TABLE, ItemId::new(9))?;
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(9)]))
        })
        .expect_err("a hint assigned in this transaction must name a held item");
    assert_item_not_found(&err, 9);
}

#[test]
fn transaction_custom_settles_hints_too() {
    let session = hinted_41();
    session
        .transaction_custom(|ui| {
            ui.apply_op(&Operation::model_delete_items(MODEL, [ItemId::new(41)]))
                .map_err(|e| e.to_string())
        })
        .expect("custom commit");
    assert_eq!(hinted(&session), None);

    let session = self::session();
    let revision = session.current_revision();
    let err = session
        .transaction_custom(|ui| {
            ui.set(TABLE, KEEP_VISIBLE, Value::ItemId(ItemId::new(999_999)))
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .expect_err("unknown item");
    assert_item_not_found(&err, 999_999);
    assert_eq!(session.current_revision(), revision);
}

#[test]
fn swallowed_settlement_refusal_is_refused_again_by_the_commit() {
    let session = session();
    let revision = session.current_revision();
    let err = session
        .transaction(|ui| {
            ui.set(TABLE, KEEP_VISIBLE, Value::ItemId(ItemId::new(999_999)))?;
            assert!(ui.settle_promoted_selections().is_err());
            Ok(())
        })
        .expect_err("the commit path settles again");
    assert_item_not_found(&err, 999_999);
    assert_eq!(session.current_revision(), revision);
}
