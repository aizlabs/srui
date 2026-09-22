//! PX-002 acceptance: injected deterministic domain records become collection rows (§§8, 12, 29).
use srui_process_explorer::{initialize_from_source, source::*, MODEL, STATUS, TABLE};
use srui_sdk::{Value, ACTIONS, ACTION_KEY, TEXT};
use srui_sessiond::Session;
use std::time::{Duration, SystemTime};

#[test]
fn fake_snapshot_is_fixed_without_sampling_the_host_or_clock() {
    let mut source = FakeProcessSource;
    let first = source.snapshot();
    assert_eq!(first, source.snapshot());
    assert_eq!(first.source, SourceId("fake-processes-v1".into()));
    assert_eq!(
        first.sampled_at,
        SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000))
    );
    assert_eq!(first.records.len(), 3);
    assert_eq!(first.records[0].display_name, first.records[1].display_name);
    assert_ne!(first.records[0].key, first.records[1].key);
    assert_ne!(first.records[0].key.creation, first.records[1].key.creation);
    assert_eq!(
        first.records[2].key.pid,
        Observed::Missing(MissingReason::Unavailable)
    );
    // A fixed fake set is authoritative: it is complete, not a degraded scan.
    assert_eq!(first.completeness, Completeness::Complete);
}

#[test]
fn injected_source_is_called_once_and_three_rows_are_model_data() {
    struct CountingSource {
        calls: usize,
    }
    impl ProcessSource for CountingSource {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            self.calls += 1;
            assert_eq!(
                self.calls, 1,
                "one-shot initialization must sample only the injected source"
            );
            FakeProcessSource.snapshot()
        }
    }
    let session = Session::mint();
    let mut source = CountingSource { calls: 0 };
    let snapshot = initialize_from_source(&session, &mut source).unwrap();
    assert_eq!(source.calls, 1);
    assert_eq!(snapshot.source, SourceId("fake-processes-v1".into()));
    assert_eq!(session.current_revision(), 1);
    session.with_store(|store| {
        assert_eq!(
            store.node_count(),
            5,
            "records must not create child view nodes"
        );
        assert_eq!(store.children_of(TABLE), Some([].as_slice()));
        let model = store.get_model(MODEL).unwrap();
        assert_eq!(model.item_count, 3);
        assert_eq!(model.cached_item_count(), 3);
        let rows: Vec<_> = model.items.values().collect();
        assert_eq!(
            rows.iter().map(|row| row.value.clone()).collect::<Vec<_>>(),
            vec![
                Value::List(vec![
                    Value::UnsignedInt(4101),
                    Value::String("worker".into())
                ]),
                Value::List(vec![
                    Value::UnsignedInt(4102),
                    Value::String("worker".into())
                ]),
                Value::List(vec![
                    Value::String("Unavailable".into()),
                    Value::String("helper".into())
                ]),
            ]
        );
        assert_eq!(model.id_to_index.len(), 3);
        assert_ne!(rows[0].item_id, rows[1].item_id);
        assert!(rows
            .iter()
            .all(|row| row.item_id.get() != 4101 && row.item_id.get() != 4102));
        for id in 1..=5 {
            let node = store.get_node(srui_sdk::NodeId::new(id)).unwrap();
            assert!(!node.has_property(ACTIONS));
            assert!(!node.has_property(ACTION_KEY));
        }
    });
}

#[test]
fn status_is_the_injected_source_description_not_a_fixed_fixture_label() {
    struct LabeledSource(&'static str);
    impl ProcessSource for LabeledSource {
        fn status_text(&self) -> &str {
            self.0
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            FakeProcessSource.snapshot()
        }
    }

    for label in ["Read-only · Live process snapshot", FAKE_STATUS_TEXT] {
        let session = Session::mint();
        initialize_from_source(&session, &mut LabeledSource(label)).unwrap();
        session.with_store(|store| {
            assert_eq!(
                store.get_node(STATUS).unwrap().get_property(TEXT),
                Some(&Value::String(label.into())),
                "non-fixture data must never be labeled as synthetic"
            );
        });
    }
}

#[test]
fn a_snapshot_larger_than_one_model_batch_is_published_whole() {
    // A live host can hold more processes than §26 allows in a single model
    // mutation batch. Publishing must split into bounded batches rather than
    // abort initialization and leave the operator with no window at all.
    struct CrowdedSource(usize);
    impl ProcessSource for CrowdedSource {
        fn status_text(&self) -> &str {
            "Read-only · Crowded fixture snapshot"
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let mut snapshot = FakeProcessSource.snapshot();
            let template = snapshot.records[0].key.clone();
            snapshot.records = (0..self.0)
                .map(|index| ProcessRecord {
                    key: ProcessKey {
                        pid: Observed::Known(index as u32 + 1),
                        creation: CreationToken::Opaque(format!("crowded-{index}")),
                        ..template.clone()
                    },
                    display_name: DisplayName::sanitize(b"worker"),
                })
                .collect();
            snapshot
        }
    }

    let session = Session::mint();
    let limit = session.with_store(|store| store.limits().max_items_per_model_operation);
    let crowded = limit * 2 + 1;
    let snapshot = initialize_from_source(&session, &mut CrowdedSource(crowded)).unwrap();
    assert_eq!(snapshot.records.len(), crowded);
    assert_eq!(
        session.current_revision(),
        1,
        "every batch belongs to the one transaction that publishes the shell"
    );
    session.with_store(|store| {
        let model = store.get_model(MODEL).unwrap();
        assert_eq!(model.item_count, crowded as u64);
        assert_eq!(model.id_to_index.len(), crowded, "no row is dropped");
        assert_eq!(store.node_count(), 5, "rows never become view nodes");
    });
}

#[test]
fn invalid_source_identity_does_not_publish_partial_ui() {
    struct DuplicateSource;
    impl ProcessSource for DuplicateSource {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let mut snapshot = FakeProcessSource.snapshot();
            snapshot.records[1].key = snapshot.records[0].key.clone();
            snapshot
        }
    }
    let session = Session::mint();
    assert!(initialize_from_source(&session, &mut DuplicateSource).is_err());
    assert_eq!(session.current_revision(), 0);
    assert_eq!(session.node_count(), 0);
    session.with_store(|store| assert!(store.get_model(MODEL).is_none()));
}
