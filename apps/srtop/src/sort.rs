//! Server-authoritative ordering of the published process collection (PX-009,
//! design §§6.2, 7.6, 8).
//!
//! Ordering is a publication concern of the server, never a client policy:
//! §8 allows local sorting only where the protocol explicitly allows it, and it
//! does not here. A sort request is a standard `ACTIVATE` on one of the four
//! column Buttons (§7.6); the server resolves it, reorders its own rows and
//! republishes the collection's order through model operations that keep every
//! row's item identity, so the client's native selection — anchored to item
//! IDs — stays on the same process (§6.2).
//!
//! Three rules shape the comparison:
//!
//! * Values are compared as what they are, never as the text a cell shows: PID
//!   and resident memory by their stored integers, CPU by its computed tenths
//!   of one CPU, and the name by its sanitized text (`9` sorts below `10`, and
//!   `9 B` below `1.0 KiB`, where their texts would say the opposite). Which of
//!   the two shapes a column takes is the catalogue's [`SortKey`]; the
//!   direction a first click applies is its [`crate::catalogue::DefaultSort`].
//! * A value that is not present — denied, unavailable, warming up, or a
//!   measurement too broken to publish, exactly the states
//!   [`Availability`] does not call `Present` — sorts last in *both*
//!   directions. It is not a very small number, exactly as it is never a
//!   published zero.
//! * Ties never depend on iteration order or history: equal values, and the
//!   missing group among themselves, are ordered by [`process_key_order`], a
//!   total order over the process-instance identity, in the same direction
//!   whichever way the values are ordered. Two rows with the same display name
//!   therefore keep a deterministic, repeatable order.
//!
//! Until the first click the collection has no sort at all: it is published in
//! the order its source supplies, exactly as before this module existed, and
//! no button is marked active.

use crate::catalogue::{Availability, ProcessField, SortDirection};
use crate::metric;
use crate::projection::Row;
use crate::source::{CpuUsage, CreationToken, Observed, ProcessKey, ProcessRecord};
use std::cmp::Ordering;

/// The collection's active ordering: one column and one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortOrder {
    pub field: ProcessField,
    pub direction: SortDirection,
}

impl SortOrder {
    /// The order a click on `field` resolves to against the `active` one: that
    /// column's catalogued default direction when it is not the active column
    /// (or nothing is sorted yet), and the opposite direction when it is — a
    /// second click on the same column reverses the order.
    pub fn resolve(active: Option<Self>, field: ProcessField) -> Self {
        match active {
            Some(active) if active.field == field => Self {
                field,
                direction: opposite(active.direction),
            },
            _ => Self {
                field,
                direction: field.spec().sort.direction,
            },
        }
    }
}

/// The other direction.
pub fn opposite(direction: SortDirection) -> SortDirection {
    match direction {
        SortDirection::Ascending => SortDirection::Descending,
        SortDirection::Descending => SortDirection::Ascending,
    }
}

/// The typed order value of one row for one column: a present quantity or
/// identifier by its stored integer, present text by its characters, and every
/// state that is not present as [`SortValue::Missing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortValue {
    /// A present PID, byte count or CPU share (tenths of one CPU's percent), by
    /// its stored integer: the number, never the formatted cell.
    Number(u64),
    /// A present sanitized name, by its text.
    Text(String),
    /// Denied, unavailable, warming up, or a measurement too broken to publish.
    Missing,
}

/// The typed order values of one row, one per catalogued column, extracted once
/// at projection from the same record the cells are formatted from. A retained
/// row — one a degraded scan did not confirm — keeps the values it was
/// published under, so it takes part in every later sort instead of sinking to
/// the end as though all its values were missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortValues {
    pid: SortValue,
    name: SortValue,
    resident: SortValue,
    cpu: SortValue,
}

impl SortValues {
    /// The order values of one record under its session-anchored key.
    pub fn of_record(key: &ProcessKey, record: &ProcessRecord) -> Self {
        Self {
            pid: value_of(ProcessField::Pid, key, record),
            name: value_of(ProcessField::Name, key, record),
            resident: value_of(ProcessField::Resident, key, record),
            cpu: value_of(ProcessField::Cpu, key, record),
        }
    }

    /// This row's order value for `field`.
    pub fn get(&self, field: ProcessField) -> &SortValue {
        match field {
            ProcessField::Pid => &self.pid,
            ProcessField::Name => &self.name,
            ProcessField::Resident => &self.resident,
            ProcessField::Cpu => &self.cpu,
        }
    }
}

/// One column's typed order value for one record.
///
/// Presence is the catalogue's to decide ([`ProcessField::availability`]), so
/// a value sorts among the numbers exactly when its cell shows a number: a CPU
/// interval that cannot be divided is `Unavailable` in its cell and missing
/// here. The shape follows the column's catalogued [`SortKey`] — numeric for the
/// PID, resident memory and CPU, lexical for the name — which
/// `every_column_orders_by_its_catalogued_key` checks against the catalogue.
pub fn value_of(field: ProcessField, key: &ProcessKey, record: &ProcessRecord) -> SortValue {
    if field.availability(key, record) != Availability::Present {
        return SortValue::Missing;
    }
    match field {
        ProcessField::Pid => match key.pid {
            Observed::Known(pid) => SortValue::Number(u64::from(pid)),
            Observed::Missing(_) => SortValue::Missing,
        },
        ProcessField::Name => SortValue::Text(record.display_name.as_str().to_string()),
        ProcessField::Resident => match record.resident {
            Observed::Known(bytes) => SortValue::Number(bytes),
            Observed::Missing(_) => SortValue::Missing,
        },
        ProcessField::Cpu => match &record.cpu {
            CpuUsage::Measured(interval) => metric::cpu_tenths_of_percent(interval)
                .map_or(SortValue::Missing, SortValue::Number),
            CpuUsage::WarmingUp | CpuUsage::Missing(_) => SortValue::Missing,
        },
    }
}

/// The order `order` imposes on two rows: by the column's typed values in the
/// requested direction, missing values last in both directions, and — for equal
/// values and among the missing alike — by [`process_key_order`], so the result
/// is a total, deterministic order.
///
/// Text compares by Unicode scalar value (the byte order of its UTF-8), case
/// sensitively: deterministic on every host, and not a locale's collation.
pub(crate) fn compare(order: SortOrder, left: &Row, right: &Row) -> Ordering {
    let compared = match (left.sort.get(order.field), right.sort.get(order.field)) {
        // A missing value is not an extreme number: it is last, whichever way
        // the present values are ordered.
        (SortValue::Missing, SortValue::Missing) => Ordering::Equal,
        (SortValue::Missing, _) => return Ordering::Greater,
        (_, SortValue::Missing) => return Ordering::Less,
        (SortValue::Number(left_value), SortValue::Number(right_value)) => {
            directed(order.direction, left_value.cmp(right_value))
        }
        (SortValue::Text(left_text), SortValue::Text(right_text)) => {
            directed(order.direction, left_text.cmp(right_text))
        }
        // One column's values take one shape (`value_of`); a mixed pair cannot
        // be constructed, and would fall through to the identity order.
        (SortValue::Number(_), SortValue::Text(_)) | (SortValue::Text(_), SortValue::Number(_)) => {
            Ordering::Equal
        }
    };
    compared.then_with(|| process_key_order(&left.key, &right.key))
}

fn directed(direction: SortDirection, ordering: Ordering) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

/// A total order over process-instance identities, independent of any scan:
/// source, host, boot, PID namespace, PID, then creation token.
///
/// It exists because the identity components carry no `Ord` — an [`Observed`]
/// holds a *reason*, which is not an ordering — and because a tie-breaker must
/// be exactly this: deterministic, repeatable, and the same in both sort
/// directions. A known component orders before a missing one, the two missing
/// reasons are equal, and creation tokens compare within their own scheme with
/// a fixed order between schemes. The projection refuses a snapshot whose keys
/// collide, so two rows compare equal here only if their keys differ in nothing
/// but the reason a component is missing; the collection is sorted stably, so
/// even those keep the order they arrived in.
pub fn process_key_order(left: &ProcessKey, right: &ProcessKey) -> Ordering {
    left.source
        .0
        .cmp(&right.source.0)
        .then_with(|| observed_order(&left.host, &right.host, |a, b| a.0.cmp(&b.0)))
        .then_with(|| observed_order(&left.boot, &right.boot, |a, b| a.0.cmp(&b.0)))
        .then_with(|| {
            observed_order(&left.pid_namespace, &right.pid_namespace, |a, b| {
                a.0.cmp(&b.0)
            })
        })
        .then_with(|| observed_order(&left.pid, &right.pid, |a, b| a.cmp(b)))
        .then_with(|| creation_order(&left.creation, &right.creation))
}

/// A known component before a missing one, and the values by `compare` when
/// both are known; the two reasons for absence are not an order.
fn observed_order<T>(
    left: &Observed<T>,
    right: &Observed<T>,
    compare: impl FnOnce(&T, &T) -> Ordering,
) -> Ordering {
    match (left, right) {
        (Observed::Known(left_value), Observed::Known(right_value)) => {
            compare(left_value, right_value)
        }
        (Observed::Known(_), Observed::Missing(_)) => Ordering::Less,
        (Observed::Missing(_), Observed::Known(_)) => Ordering::Greater,
        (Observed::Missing(_), Observed::Missing(_)) => Ordering::Equal,
    }
}

/// Tokens of one scheme compare by value; tokens of different schemes never
/// pretend to, and take a fixed order between them instead.
fn creation_order(left: &CreationToken, right: &CreationToken) -> Ordering {
    match (left, right) {
        (CreationToken::LinuxBootTicks(left_ticks), CreationToken::LinuxBootTicks(right_ticks)) => {
            left_ticks.cmp(right_ticks)
        }
        (CreationToken::Opaque(left_token), CreationToken::Opaque(right_token)) => {
            left_token.cmp(right_token)
        }
        (CreationToken::LinuxBootTicks(_), CreationToken::Opaque(_)) => Ordering::Less,
        (CreationToken::Opaque(_), CreationToken::LinuxBootTicks(_)) => Ordering::Greater,
    }
}

/// The label of one column's sort button: its plain name, with the direction
/// mark when this column is the active order.
pub fn sort_button_label(field: ProcessField, active: Option<SortDirection>) -> String {
    let name = field.button_name();
    match active {
        None => format!("Sort by {name}"),
        Some(SortDirection::Ascending) => format!("Sort by {name} \u{2191}"),
        Some(SortDirection::Descending) => format!("Sort by {name} \u{2193}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::{SortKey, PROCESS_LIST};
    use crate::source::{BootId, CpuInterval, HostId, MissingReason, PidNamespaceId, SourceId};

    fn key(pid: Option<u32>, token: &str) -> ProcessKey {
        ProcessKey {
            source: SourceId("test-source".into()),
            host: Observed::Known(HostId("host".into())),
            boot: Observed::Known(BootId("boot".into())),
            pid_namespace: Observed::Known(PidNamespaceId(1)),
            pid: pid.map_or(Observed::Missing(MissingReason::Denied), Observed::Known),
            creation: CreationToken::Opaque(token.into()),
        }
    }

    /// A row for item `id`, projected from a record exactly as the collection
    /// projects it.
    fn row(
        id: u64,
        pid: Option<u32>,
        token: &str,
        name: &str,
        resident: Option<u64>,
        cpu: CpuUsage,
    ) -> Row {
        let key = key(pid, token);
        let record = ProcessRecord {
            key: key.clone(),
            display_name: name.into(),
            resident: resident.map_or(
                Observed::Missing(MissingReason::Unavailable),
                Observed::Known,
            ),
            cpu,
        };
        Row {
            sort: SortValues::of_record(&key, &record),
            key,
            item_id: srui_sdk::ItemId::new(id),
            cells: Vec::new(),
            value: srui_sdk::Value::UnsignedInt(id),
            display_name: record.display_name,
        }
    }

    fn measured(ticks: u64) -> CpuUsage {
        CpuUsage::Measured(CpuInterval {
            ticks,
            ticks_per_second: 100,
            elapsed: std::time::Duration::from_secs(1),
        })
    }

    fn order(field: ProcessField, direction: SortDirection) -> SortOrder {
        SortOrder { field, direction }
    }

    fn ids(order: SortOrder, rows: &[Row]) -> Vec<u64> {
        let mut sorted = rows.to_vec();
        sorted.sort_by(|left, right| compare(order, left, right));
        sorted.iter().map(|row| row.item_id.get()).collect()
    }

    const BOTH: [SortDirection; 2] = [SortDirection::Ascending, SortDirection::Descending];

    #[test]
    fn every_column_orders_by_its_catalogued_key() {
        // A record with every field present: each value takes the shape its
        // column's catalogued sort key names.
        let present = row(1, Some(7), "t", "name", Some(9), measured(5));
        for field in PROCESS_LIST {
            let shape = match present.sort.get(field) {
                SortValue::Number(_) => SortKey::Numeric,
                SortValue::Text(_) => SortKey::Lexical,
                SortValue::Missing => panic!("{field:?} is present in this record"),
            };
            assert_eq!(shape, field.spec().sort.key, "{field:?}");
        }
        // And a value is missing exactly when the catalogue says it is not
        // present.
        let missing = row(2, None, "u", "name", None, CpuUsage::WarmingUp);
        let record = ProcessRecord {
            key: missing.key.clone(),
            display_name: "name".into(),
            resident: Observed::Missing(MissingReason::Unavailable),
            cpu: CpuUsage::WarmingUp,
        };
        for field in PROCESS_LIST {
            assert_eq!(
                *missing.sort.get(field) == SortValue::Missing,
                field.availability(&missing.key, &record) != Availability::Present,
                "{field:?}"
            );
        }
    }

    #[test]
    fn numbers_order_numerically_never_as_their_text() {
        // PID 9 below 100, resident 9 bytes below 1024, CPU 2% below 10% — the
        // opposite of their formatted texts' byte order ("100" < "9",
        // "1.0 KiB" < "9 B", "10.0%" < "2.0%").
        let rows = vec![
            row(100, Some(100), "a", "a", Some(1024), measured(10)),
            row(9, Some(9), "b", "b", Some(9), measured(2)),
        ];
        for field in [ProcessField::Pid, ProcessField::Resident, ProcessField::Cpu] {
            assert_eq!(
                ids(order(field, SortDirection::Ascending), &rows),
                [9, 100],
                "{field:?}"
            );
            assert_eq!(
                ids(order(field, SortDirection::Descending), &rows),
                [100, 9],
                "{field:?}"
            );
        }
        // Text orders lexically: `a10` before `a9`, as their characters do.
        let rows = vec![
            row(2, Some(2), "x", "a9", Some(0), measured(0)),
            row(1, Some(1), "y", "a10", Some(0), measured(0)),
        ];
        assert_eq!(
            ids(order(ProcessField::Name, SortDirection::Ascending), &rows),
            [1, 2]
        );
        assert_eq!(
            ids(order(ProcessField::Name, SortDirection::Descending), &rows),
            [2, 1]
        );
    }

    #[test]
    fn missing_values_sort_last_in_both_directions() {
        let rows = vec![
            // Every kind of not-present: a denied PID, unread memory, a CPU
            // share warming up, and a measurement whose interval cannot be
            // divided (zero elapsed), which its cell publishes as unavailable.
            row(1, None, "denied-pid", "a", Some(5), measured(1)),
            row(2, Some(2), "unread-memory", "b", None, measured(1)),
            row(3, Some(3), "warming-up", "c", Some(5), CpuUsage::WarmingUp),
            row(
                4,
                Some(4),
                "broken-measurement",
                "d",
                Some(5),
                CpuUsage::Measured(CpuInterval {
                    ticks: 50,
                    ticks_per_second: 100,
                    elapsed: std::time::Duration::ZERO,
                }),
            ),
            // A known zero is a value, not a missing one.
            row(5, Some(5), "zero", "e", Some(0), measured(0)),
        ];
        // PID: the present values order both ways; the denied PID is last.
        assert_eq!(
            ids(order(ProcessField::Pid, SortDirection::Ascending), &rows),
            [2, 3, 4, 5, 1]
        );
        assert_eq!(
            ids(order(ProcessField::Pid, SortDirection::Descending), &rows),
            [5, 4, 3, 2, 1]
        );
        // Resident: the known zero sorts as zero; the unread value is last
        // both ways; the three equal values tie to the identity order (the
        // denied PID's key after the known ones).
        assert_eq!(
            ids(
                order(ProcessField::Resident, SortDirection::Ascending),
                &rows
            ),
            [5, 3, 4, 1, 2]
        );
        assert_eq!(
            ids(
                order(ProcessField::Resident, SortDirection::Descending),
                &rows
            ),
            [3, 4, 1, 5, 2]
        );
        // CPU: the warming-up and broken measurements are last in both
        // directions, among themselves in identity order.
        assert_eq!(
            ids(order(ProcessField::Cpu, SortDirection::Ascending), &rows),
            [5, 2, 1, 3, 4]
        );
        assert_eq!(
            ids(order(ProcessField::Cpu, SortDirection::Descending), &rows),
            [2, 1, 5, 3, 4]
        );
    }

    #[test]
    fn ties_break_by_process_key_the_same_way_in_both_directions() {
        // Same name, memory and CPU: only the identity tells them apart, and it
        // tells them apart the same way in both directions and whatever order
        // the rows arrive in.
        let rows = vec![
            row(20, Some(20), "second", "same", Some(10), measured(3)),
            row(10, Some(10), "first", "same", Some(10), measured(3)),
            row(
                30,
                Some(10),
                "another-instance",
                "same",
                Some(10),
                measured(3),
            ),
        ];
        let mut reversed = rows.clone();
        reversed.reverse();
        for field in [
            ProcessField::Name,
            ProcessField::Resident,
            ProcessField::Cpu,
        ] {
            for direction in BOTH {
                // PID 10 twice — a reused PID — is told apart by its creation
                // token: "another-instance" before "first".
                assert_eq!(
                    ids(order(field, direction), &rows),
                    [30, 10, 20],
                    "{field:?} {direction:?}"
                );
                assert_eq!(ids(order(field, direction), &reversed), [30, 10, 20]);
            }
        }
    }

    #[test]
    fn the_key_order_covers_every_component_and_stays_total() {
        let base = row(1, Some(1), "a", "a", None, CpuUsage::WarmingUp);
        let mutate: [fn(&mut Row); 6] = [
            |row| row.key.source = SourceId("another".into()),
            |row| row.key.host = Observed::Missing(MissingReason::Denied),
            |row| row.key.boot = Observed::Missing(MissingReason::Unavailable),
            |row| row.key.pid_namespace = Observed::Known(PidNamespaceId(7)),
            |row| row.key.pid = Observed::Known(2),
            |row| row.key.creation = CreationToken::LinuxBootTicks(42),
        ];
        for change in mutate {
            let mut other = base.clone();
            change(&mut other);
            let forward = process_key_order(&base.key, &other.key);
            assert_ne!(
                forward,
                Ordering::Equal,
                "a distinct key must never compare equal"
            );
            assert_eq!(process_key_order(&other.key, &base.key), forward.reverse());
        }
        // The two missing reasons are equal, not ordered.
        let mut denied = base.clone();
        denied.key.host = Observed::Missing(MissingReason::Denied);
        let mut unavailable = base.clone();
        unavailable.key.host = Observed::Missing(MissingReason::Unavailable);
        assert_eq!(
            process_key_order(&denied.key, &unavailable.key),
            Ordering::Equal
        );
    }

    #[test]
    fn a_click_resolves_to_the_catalogued_default_then_toggles() {
        // Nothing sorted yet: every column takes its catalogued direction —
        // the PID and the name ascending, resident memory and CPU descending.
        for (field, direction) in [
            (ProcessField::Pid, SortDirection::Ascending),
            (ProcessField::Name, SortDirection::Ascending),
            (ProcessField::Resident, SortDirection::Descending),
            (ProcessField::Cpu, SortDirection::Descending),
        ] {
            assert_eq!(direction, field.spec().sort.direction);
            assert_eq!(SortOrder::resolve(None, field), order(field, direction));
        }
        // The active column reverses, both ways.
        let cpu = SortOrder::resolve(None, ProcessField::Cpu);
        let reversed = SortOrder::resolve(Some(cpu), ProcessField::Cpu);
        assert_eq!(reversed, order(ProcessField::Cpu, SortDirection::Ascending));
        assert_eq!(SortOrder::resolve(Some(reversed), ProcessField::Cpu), cpu);
        // A column left and revisited takes its default again, not a memory of
        // its last direction.
        let away = SortOrder::resolve(Some(reversed), ProcessField::Name);
        assert_eq!(away, order(ProcessField::Name, SortDirection::Ascending));
        assert_eq!(SortOrder::resolve(Some(away), ProcessField::Cpu), cpu);
    }

    #[test]
    fn labels_name_the_column_and_mark_the_active_direction() {
        assert_eq!(sort_button_label(ProcessField::Pid, None), "Sort by PID");
        assert_eq!(sort_button_label(ProcessField::Cpu, None), "Sort by CPU");
        assert_eq!(
            sort_button_label(ProcessField::Pid, Some(SortDirection::Ascending)),
            "Sort by PID \u{2191}"
        );
        assert_eq!(
            sort_button_label(ProcessField::Resident, Some(SortDirection::Descending)),
            "Sort by Resident \u{2193}"
        );
        // The button name is the heading's leading word, never another name.
        for field in PROCESS_LIST {
            assert!(
                field.heading().starts_with(field.button_name()),
                "{field:?}"
            );
        }
    }
}
