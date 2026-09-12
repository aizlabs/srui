//! Server-side row projection and session-local item allocation (§§6.2, 8; PX-002).
use crate::source::{MissingReason, Observed, ProcessSnapshot, SourceId, SourceRecordId};
use srui_sdk::{ItemId, Value};
use srui_semantic_tree::ModelItem;
use std::collections::{HashMap, HashSet};
use std::io;

/// Allocate independently of PID, display name, and row position. Entries are never removed,
/// so an item ID is never reused in this allocator's semantic session.
#[derive(Default)]
pub(crate) struct SessionItemIds {
    assigned: HashMap<(SourceId, SourceRecordId), ItemId>,
    last_id: u64,
}

impl SessionItemIds {
    pub(crate) fn project(&mut self, snapshot: &ProcessSnapshot) -> io::Result<Vec<ModelItem>> {
        let mut seen = HashSet::with_capacity(snapshot.records.len());
        if snapshot
            .records
            .iter()
            .any(|record| !seen.insert(&record.id))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate source record identity",
            ));
        }
        snapshot
            .records
            .iter()
            .map(|record| {
                let key = (snapshot.source.clone(), record.id.clone());
                let item_id = match self.assigned.get(&key) {
                    Some(id) => *id,
                    None => {
                        self.last_id = self
                            .last_id
                            .checked_add(1)
                            .ok_or_else(|| io::Error::other("session item IDs exhausted"))?;
                        let id = ItemId::new(self.last_id);
                        self.assigned.insert(key, id);
                        id
                    }
                };
                let pid = match record.pid {
                    Observed::Known(pid) => Value::UnsignedInt(u64::from(pid)),
                    Observed::Missing(MissingReason::Unavailable) => {
                        Value::String("Unavailable".into())
                    }
                    Observed::Missing(MissingReason::Denied) => Value::String("Denied".into()),
                };
                Ok(ModelItem::with_value(
                    item_id,
                    Value::List(vec![pid, Value::String(record.display_name.clone())]),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{FakeProcessSource, ProcessSource};

    #[test]
    fn identity_survives_reorder_rename_pid_change_and_source_changes_do_not_alias() {
        let mut snapshot = FakeProcessSource.snapshot();
        let mut ids = SessionItemIds::default();
        let original = ids.project(&snapshot).unwrap();
        snapshot.records.reverse();
        snapshot.records[2].display_name = "renamed".into();
        snapshot.records[2].pid = Observed::Known(99);
        let reordered = ids.project(&snapshot).unwrap();
        assert_eq!(reordered[2].item_id, original[0].item_id);
        assert_eq!(reordered[0].item_id, original[2].item_id);
        snapshot.source = SourceId("another-source".into());
        let other_source = ids.project(&snapshot).unwrap();
        assert!(other_source
            .iter()
            .all(|row| original.iter().all(|old| old.item_id != row.item_id)));
        snapshot.records[0].id = SourceRecordId("new-instance".into());
        let new_instance = ids.project(&snapshot).unwrap();
        assert_ne!(new_instance[0].item_id, other_source[0].item_id);
    }

    #[test]
    fn duplicate_identity_is_rejected_before_allocating() {
        let mut snapshot = FakeProcessSource.snapshot();
        snapshot.records[1].id = snapshot.records[0].id.clone();
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
        snapshot.records[0].pid = Observed::Known(0);
        snapshot.records[1].pid = Observed::Missing(MissingReason::Denied);
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
}
