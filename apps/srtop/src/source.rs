//! Typed, injectable one-shot process snapshots (design §§6.3, 8; PX-002).
//! This fake adapter uses constants only: no OS process enumeration or clock reads.
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

/// An opaque token supplied by a source, not a PID or display name.
/// Live OS process-instance identity is the separate PX-003 contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRecordId(pub String);

/// UTC wall-clock sample time; never a process creation/identity token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotTime(pub SystemTime);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingReason {
    Unavailable,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed<T> {
    Known(T),
    Missing(MissingReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRecord {
    pub id: SourceRecordId,
    pub pid: Observed<u32>,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub source: SourceId,
    pub sampled_at: SnapshotTime,
    /// Authoritative display order, supplied by the server-side source.
    pub records: Vec<ProcessRecord>,
}

pub trait ProcessSource {
    fn snapshot(&mut self) -> ProcessSnapshot;
}

#[derive(Debug, Default)]
pub struct FakeProcessSource;

impl ProcessSource for FakeProcessSource {
    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            source: SourceId("fake-processes-v1".into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            records: vec![
                ProcessRecord {
                    id: SourceRecordId("worker-a".into()),
                    pid: Observed::Known(4101),
                    display_name: "worker".into(),
                },
                ProcessRecord {
                    id: SourceRecordId("worker-b".into()),
                    pid: Observed::Known(4102),
                    display_name: "worker".into(),
                },
                ProcessRecord {
                    id: SourceRecordId("helper".into()),
                    pid: Observed::Missing(MissingReason::Unavailable),
                    display_name: "helper".into(),
                },
            ],
        }
    }
}
