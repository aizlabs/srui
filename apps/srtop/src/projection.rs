//! Server-side row projection and session-local item allocation
//! (§§6.2, 8; PX-002 rows, PX-003 process-instance keys).
use crate::source::{MissingReason, Observed, ProcessKey, ProcessSnapshot};
use srui_sdk::{ItemId, Value};
use srui_semantic_tree::ModelItem;
use std::collections::{HashMap, HashSet};
use std::io;

/// Resolves a full [`ProcessKey`] to an opaque item ID. Allocation is
/// independent of PID alone, display name and row position: a reused PID with a
/// different creation token is a different key and receives a different ID.
/// Entries are never removed, so an item ID is never reused within this
/// allocator's semantic session.
#[derive(Default)]
pub(crate) struct SessionItemIds {
    assigned: HashMap<ProcessKey, ItemId>,
    last_id: u64,
}

impl SessionItemIds {
    pub(crate) fn project(&mut self, snapshot: &ProcessSnapshot) -> io::Result<Vec<ModelItem>> {
        let mut seen = HashSet::with_capacity(snapshot.records.len());
        if snapshot
            .records
            .iter()
            .any(|record| !seen.insert(&record.key))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate process instance identity",
            ));
        }
        snapshot
            .records
            .iter()
            .map(|record| {
                let item_id = match self.assigned.get(&record.key) {
                    Some(id) => *id,
                    None => {
                        self.last_id = self
                            .last_id
                            .checked_add(1)
                            .ok_or_else(|| io::Error::other("session item IDs exhausted"))?;
                        let id = ItemId::new(self.last_id);
                        self.assigned.insert(record.key.clone(), id);
                        id
                    }
                };
                let pid = match record.key.pid {
                    Observed::Known(pid) => Value::UnsignedInt(u64::from(pid)),
                    Observed::Missing(MissingReason::Unavailable) => {
                        Value::String("Unavailable".into())
                    }
                    Observed::Missing(MissingReason::Denied) => Value::String("Denied".into()),
                };
                Ok(ModelItem::with_value(
                    item_id,
                    Value::List(vec![
                        pid,
                        Value::String(record.display_name.as_str().to_string()),
                    ]),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{CreationToken, FakeProcessSource, ProcessSource, SourceId};

    #[test]
    fn identity_survives_reorder_and_rename_and_never_aliases_across_sources() {
        let mut snapshot = FakeProcessSource.snapshot();
        let mut ids = SessionItemIds::default();
        let original = ids.project(&snapshot).unwrap();
        snapshot.records.reverse();
        snapshot.records[2].display_name = "renamed".into();
        let reordered = ids.project(&snapshot).unwrap();
        assert_eq!(reordered[2].item_id, original[0].item_id);
        assert_eq!(reordered[0].item_id, original[2].item_id);
        snapshot.source = SourceId("another-source".into());
        for record in &mut snapshot.records {
            record.key.source = SourceId("another-source".into());
        }
        let other_source = ids.project(&snapshot).unwrap();
        assert!(other_source
            .iter()
            .all(|row| original.iter().all(|old| old.item_id != row.item_id)));
    }

    #[test]
    fn same_pid_with_a_different_creation_token_gets_a_new_item_id() {
        let mut snapshot = FakeProcessSource.snapshot();
        let mut ids = SessionItemIds::default();
        let original = ids.project(&snapshot).unwrap();
        // Same PID, same name, same host/boot/namespace: only the creation token
        // differs, which is exactly a reused PID after the first instance exited.
        snapshot.records[0].key.creation = CreationToken::Opaque("worker-a-restarted".into());
        let reused = ids.project(&snapshot).unwrap();
        assert_ne!(reused[0].item_id, original[0].item_id);
        assert_eq!(reused[0].value, original[0].value);
        assert_eq!(reused[1].item_id, original[1].item_id);
        // The retired ID is never handed back out.
        assert!(reused.iter().all(|row| row.item_id != original[0].item_id));
    }

    #[test]
    fn boot_host_namespace_and_pid_are_all_discriminators() {
        use crate::source::{BootId, HostId, PidNamespaceId};
        let base = FakeProcessSource.snapshot();
        let mut ids = SessionItemIds::default();
        let original = ids.project(&base).unwrap();
        let mut previous = original[0].item_id;
        let mutate: [fn(&mut ProcessKey); 4] = [
            |key| key.boot = Observed::Known(BootId("fake-boot-0002".into())),
            |key| key.host = Observed::Known(HostId("other-host".into())),
            |key| key.pid_namespace = Observed::Known(PidNamespaceId(4_026_531_999)),
            |key| key.pid = Observed::Known(9999),
        ];
        for change in mutate {
            let mut snapshot = base.clone();
            change(&mut snapshot.records[0].key);
            let rows = ids.project(&snapshot).unwrap();
            assert_ne!(rows[0].item_id, original[0].item_id);
            assert_ne!(rows[0].item_id, previous);
            previous = rows[0].item_id;
        }
    }

    #[test]
    fn duplicate_identity_is_rejected_before_allocating() {
        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.records[1].key = snapshot.records[0].key.clone();
        let mut ids = SessionItemIds::default();
        assert_eq!(
            ids.project(&snapshot).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(ids.assigned.is_empty());
        assert_eq!(ids.last_id, 0);
    }

    #[test]
    fn missing_and_denied_are_not_zero_but_known_zero_is_preserved() {
        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.records[0].key.pid = Observed::Known(0);
        snapshot.records[1].key.pid = Observed::Missing(MissingReason::Denied);
        let rows = SessionItemIds::default().project(&snapshot).unwrap();
        for (row, expected) in rows.iter().zip([
            Value::UnsignedInt(0),
            Value::String("Denied".into()),
            Value::String("Unavailable".into()),
        ]) {
            let Value::List(cells) = &row.value else {
                panic!("expected table cells")
            };
            assert_eq!(cells[0], expected);
        }
    }

    #[test]
    fn hostile_names_reach_the_model_as_inert_sanitized_text() {
        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.records[0].display_name =
            crate::source::DisplayName::sanitize(b"ev\x07il ) $(reboot) (\xff");
        let rows = SessionItemIds::default().project(&snapshot).unwrap();
        let Value::List(cells) = &rows[0].value else {
            panic!("expected table cells")
        };
        assert_eq!(
            cells[1],
            Value::String("ev\u{fffd}il ) $(reboot) (\u{fffd}".into())
        );
        let Value::String(name) = &cells[1] else {
            panic!("expected a plain string cell")
        };
        assert!(!name.chars().any(char::is_control));
    }
}
