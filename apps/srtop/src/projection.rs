//! Server-side row projection and session-local item allocation
//! (§§6.2, 8; PX-002 rows, PX-003 process-instance keys, PX-004 refresh).
use crate::source::{
    BootId, HostId, MissingReason, Observed, PidNamespaceId, ProcessKey, ProcessSnapshot,
};
use srui_sdk::{ItemId, Value};
use srui_semantic_tree::ModelItem;
use std::collections::{HashMap, HashSet};
use std::io;

/// One projected row: the process instance it was projected from, the opaque
/// item ID that instance owns, and the exact value published to the model.
///
/// The key travels with the row because a refresh publishes rows the current
/// scan did not confirm (PX-004): the allocator must be able to tell an identity
/// that is still on screen from one that is not.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub(crate) key: ProcessKey,
    pub(crate) item_id: ItemId,
    pub(crate) value: Value,
}

impl Row {
    pub(crate) fn to_model_item(&self) -> ModelItem {
        ModelItem::with_value(self.item_id, self.value.clone())
    }
}

/// The global identity of the scanned source — host, boot and PID namespace —
/// as this session has observed it (PX-004 review follow-up).
///
/// Those three components are facts about the source, not about a record: one
/// hostname per UTS namespace, one boot ID per running kernel, one namespace per
/// procfs mount (PX-003). A scan reads each of them once and stamps every record
/// it publishes with the answer, so a single unreadable `sys/kernel/hostname`
/// turns every [`ProcessKey`] of an otherwise unchanged host into a new key —
/// and, because a scan that lost an identity file skipped no PID, retention has
/// nothing to keep. Every row would be deleted and reinserted under a fresh item
/// ID, losing native row identity and selection, and again when the file came
/// back.
///
/// So the last value each component was *observed* to hold is remembered, and a
/// scan that could not read one is keyed with it. This is deliberately not a
/// claim that the component was observed on this scan: nothing remembered here
/// reaches the wire. A key is server-side row identity only — the published row
/// carries a PID and a name — and the status line still reports the degradation
/// from the scan's own issues, so a client sees "host identity incomplete" on
/// exactly the tick that saw it.
///
/// A component that comes back *different* is not a degradation and is never
/// papered over: a different host, boot or namespace is a different source, its
/// records are different process instances, and they legitimately take new
/// identities. The newly observed value is adopted, so a later degraded scan
/// anchors to the source that is actually there.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct SessionIdentity {
    host: Option<HostId>,
    boot: Option<BootId>,
    pid_namespace: Option<PidNamespaceId>,
}

impl SessionIdentity {
    /// Anchors one record's key to this session's identity, adopting whatever
    /// the scan did observe.
    fn anchor(&mut self, mut key: ProcessKey) -> ProcessKey {
        anchor_component(&mut self.host, &mut key.host);
        anchor_component(&mut self.boot, &mut key.boot);
        anchor_component(&mut self.pid_namespace, &mut key.pid_namespace);
        key
    }
}

/// Remembers an observed component, or restores one this scan could not read.
///
/// Both ways of not reading it are the same thing here: a component that became
/// unavailable and one that became permission-denied are each a scan that lost
/// sight of a fact that did not change, not a source that became a different
/// one. A component never observed at all stays missing — there is nothing to
/// restore, and inventing one would be a claim, not a memory.
fn anchor_component<T: Clone>(remembered: &mut Option<T>, observed: &mut Observed<T>) {
    match observed {
        Observed::Known(value) => *remembered = Some(value.clone()),
        Observed::Missing(_) => {
            if let Some(known) = remembered.clone() {
                *observed = Observed::Known(known);
            }
        }
    }
}

/// Resolves a full [`ProcessKey`] to an opaque item ID. Allocation is
/// independent of PID alone, display name and row position: a reused PID with a
/// different creation token is a different key and receives a different ID.
///
/// An item ID is never reused within this allocator's semantic session:
/// [`Self::retain`] may forget a key that is no longer published, but the
/// counter only ever moves forward, so a forgotten key that somehow returns is
/// given a new ID rather than an old row's.
///
/// The keys it allocates against are anchored to the session's own
/// [`SessionIdentity`] first, so a scan that temporarily lost an identity file
/// re-keys nothing and churns no row.
#[derive(Default)]
pub(crate) struct SessionItemIds {
    assigned: HashMap<ProcessKey, ItemId>,
    identity: SessionIdentity,
    last_id: u64,
}

impl SessionItemIds {
    /// Forgets every identity that is not in `published`.
    ///
    /// A poll loop samples forever, so remembering every process that ever ran
    /// would grow without bound on a host with ordinary churn. Only identities
    /// that are still on screen — including rows a degraded scan failed to
    /// confirm — need an assignment, because only those can be updated in place.
    pub(crate) fn retain(&mut self, published: &[Row]) {
        if self.assigned.len() == published.len() {
            return;
        }
        let live: HashSet<&ProcessKey> = published.iter().map(|row| &row.key).collect();
        self.assigned.retain(|key, _| live.contains(key));
    }

    /// Number of identities currently holding an assignment.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.assigned.len()
    }

    pub(crate) fn project(&mut self, snapshot: &ProcessSnapshot) -> io::Result<Vec<Row>> {
        // The keys are anchored before anything is looked up, on a copy of the
        // session identity, so a snapshot this projection rejects leaves the
        // remembered identity exactly as it was.
        let mut identity = self.identity.clone();
        let keys: Vec<ProcessKey> = snapshot
            .records
            .iter()
            .map(|record| identity.anchor(record.key.clone()))
            .collect();
        let mut seen = HashSet::with_capacity(keys.len());
        if keys.iter().any(|key| !seen.insert(key)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate process instance identity",
            ));
        }
        // Accepted: what this scan observed of the source's global identity is
        // what later scans anchor to.
        self.identity = identity;
        snapshot
            .records
            .iter()
            .zip(keys)
            .map(|(record, key)| {
                let item_id = match self.assigned.get(&key) {
                    Some(id) => *id,
                    None => {
                        self.last_id = self
                            .last_id
                            .checked_add(1)
                            .ok_or_else(|| io::Error::other("session item IDs exhausted"))?;
                        let id = ItemId::new(self.last_id);
                        self.assigned.insert(key.clone(), id);
                        id
                    }
                };
                let pid = match key.pid {
                    Observed::Known(pid) => Value::UnsignedInt(u64::from(pid)),
                    Observed::Missing(MissingReason::Unavailable) => {
                        Value::String("Unavailable".into())
                    }
                    Observed::Missing(MissingReason::Denied) => Value::String("Denied".into()),
                };
                Ok(Row {
                    key,
                    item_id,
                    value: Value::List(vec![
                        pid,
                        Value::String(record.display_name.as_str().to_string()),
                    ]),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{CreationToken, FakeProcessSource, ProcessSource, SourceId};

    /// One way a scan can answer for a global identity component.
    type KeyChange = fn(&mut ProcessKey);

    /// Replaces each record's global identity components with `change`.
    fn globally(snapshot: &mut ProcessSnapshot, change: impl Fn(&mut ProcessKey)) {
        for record in &mut snapshot.records {
            change(&mut record.key);
        }
    }

    #[test]
    fn a_global_identity_the_scan_could_not_read_keeps_the_keys_it_had() {
        let base = FakeProcessSource.snapshot();
        let lose: [KeyChange; 6] = [
            |key| key.host = Observed::Missing(MissingReason::Unavailable),
            |key| key.boot = Observed::Missing(MissingReason::Unavailable),
            |key| key.pid_namespace = Observed::Missing(MissingReason::Unavailable),
            // Denied is the same kind of event: a scan that lost sight of a fact
            // that did not change, not a different source.
            |key| key.host = Observed::Missing(MissingReason::Denied),
            |key| key.boot = Observed::Missing(MissingReason::Denied),
            |key| key.pid_namespace = Observed::Missing(MissingReason::Denied),
        ];
        for lost in lose {
            let mut ids = SessionItemIds::default();
            let original = ids.project(&base).unwrap();
            let mut degraded = base.clone();
            globally(&mut degraded, lost);
            let rows = ids.project(&degraded).unwrap();
            assert_eq!(
                rows.iter().map(|row| row.item_id).collect::<Vec<_>>(),
                original.iter().map(|row| row.item_id).collect::<Vec<_>>(),
                "an unreadable identity file re-keys nothing"
            );
            assert_eq!(ids.tracked(), original.len(), "and allocates nothing");
            // Recovery is not a change either.
            let recovered = ids.project(&base).unwrap();
            assert_eq!(recovered, original);
        }
    }

    #[test]
    fn a_global_identity_that_answers_differently_is_a_different_source() {
        use crate::source::{BootId, HostId, PidNamespaceId};
        let base = FakeProcessSource.snapshot();
        let changes: [(KeyChange, KeyChange); 3] = [
            (
                |key| key.host = Observed::Missing(MissingReason::Unavailable),
                |key| key.host = Observed::Known(HostId("another-host".into())),
            ),
            (
                |key| key.boot = Observed::Missing(MissingReason::Unavailable),
                |key| key.boot = Observed::Known(BootId("fake-boot-0002".into())),
            ),
            (
                |key| key.pid_namespace = Observed::Missing(MissingReason::Unavailable),
                |key| key.pid_namespace = Observed::Known(PidNamespaceId(4_026_531_999)),
            ),
        ];
        for (lost, changed) in changes {
            let mut ids = SessionItemIds::default();
            let original = ids.project(&base).unwrap();
            let mut degraded = base.clone();
            globally(&mut degraded, lost);
            ids.project(&degraded).unwrap();
            // The remembered value exists and is not reused: a value that was
            // read, and disagrees, is another source's.
            let mut moved = base.clone();
            globally(&mut moved, changed);
            let rows = ids.project(&moved).unwrap();
            assert!(
                rows.iter()
                    .all(|row| original.iter().all(|old| old.item_id != row.item_id)),
                "a different identity value never inherits a row"
            );
            // What was observed last is what a later degraded scan anchors to.
            let mut lost_again = moved.clone();
            globally(&mut lost_again, lost);
            let again = ids.project(&lost_again).unwrap();
            assert_eq!(
                again.iter().map(|row| row.item_id).collect::<Vec<_>>(),
                rows.iter().map(|row| row.item_id).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_global_identity_never_observed_is_not_invented() {
        let mut first = FakeProcessSource.snapshot();
        globally(&mut first, |key| {
            key.host = Observed::Missing(MissingReason::Denied)
        });
        let mut ids = SessionItemIds::default();
        let rows = ids.project(&first).unwrap();
        assert!(
            rows.iter()
                .all(|row| row.key.host == Observed::Missing(MissingReason::Denied)),
            "there is nothing to remember, and a memory is never invented"
        );
    }

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
    fn forgetting_an_unpublished_identity_never_hands_its_id_back_out() {
        let snapshot = FakeProcessSource.snapshot();
        let mut ids = SessionItemIds::default();
        let original = ids.project(&snapshot).unwrap();
        assert_eq!(ids.tracked(), 3);
        // Only the first row is still on screen: the other two identities are
        // gone from the collection and must not be remembered forever.
        ids.retain(&original[..1]);
        assert_eq!(ids.tracked(), 1);
        let again = ids.project(&snapshot).unwrap();
        assert_eq!(
            again[0].item_id, original[0].item_id,
            "a published identity keeps its row"
        );
        // A forgotten identity is a new row, never a recycled ID.
        assert_ne!(again[1].item_id, original[1].item_id);
        assert_ne!(again[2].item_id, original[2].item_id);
        assert!(again[1].item_id.get() > original[2].item_id.get());
    }

    #[test]
    fn churn_does_not_grow_the_allocator_without_bound() {
        let base = FakeProcessSource.snapshot();
        let mut ids = SessionItemIds::default();
        let mut highest = 0;
        for tick in 0..200u32 {
            let mut snapshot = base.clone();
            // Every process is replaced on every tick: the worst case for an
            // allocator that never forgets.
            for (index, record) in snapshot.records.iter_mut().enumerate() {
                record.key.creation = CreationToken::Opaque(format!("churn-{tick}-{index}"));
            }
            let rows = ids.project(&snapshot).unwrap();
            ids.retain(&rows);
            assert_eq!(ids.tracked(), rows.len());
            let lowest = rows.iter().map(|row| row.item_id.get()).min().unwrap();
            assert!(
                lowest > highest,
                "a replaced instance must never reuse an ID"
            );
            highest = rows.iter().map(|row| row.item_id.get()).max().unwrap();
        }
        assert_eq!(ids.tracked(), 3);
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
        assert!(!name.chars().any(crate::source::DisplayName::is_unsafe));
    }
}
