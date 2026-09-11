//! Connection and traffic statistics, published as ordinary semantic state (§12.2, §19, §23, §26).
//!
//! # What this can and cannot measure
//!
//! Everything here is observed **above** [`srui_sessiond::Session`], which is the highest layer an
//! application can see. That makes the following exact:
//!
//! - **Throughput** — every committed operation list is re-encoded into the same
//!   `SruiMessage{Transaction}` envelope and varint length-delimited frame `handle_connection`
//!   would write, so `framed_bytes` is the real SRUI frame size before SSH encryption (§26).
//! - **Server handling latency** — wall time from entering an event handler to the commit that
//!   settles it. This is server-side work only.
//! - **Revision lag** — actual event-transaction base minus `event.observed_revision`: how stale
//!   the client's view was at the moment it acted (§7.7).
//! - **Session facts** — attached connections, exact transaction revisions, outbound queue
//!   capacity, retained per-client state (§18.1, §20.2).
//! - **Resource transfer** — published byte count and the exact chunk count it decomposes into
//!   (§14, §19.2).
//!
//! It is **not** round-trip latency. The server never sees the client's clock, and `EVENT_ACK`,
//! handshake, framing, and `RESOURCE_*` chunk frames are emitted by the connection layer beneath
//! `Session`. Nothing in this module claims otherwise, and the gallery labels the panel
//! "server-side" for exactly that reason.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use srui_sdk::{NodeId, Operation, Progress, StoreError, UiTransaction, Value};
use srui_semantic_tree::{Revision, Transaction};
use srui_sessiond::{Session, TransactionRevisions};

use crate::ids;

/// Upper bound (inclusive) and label of each framed-transaction-size bucket.
pub const SIZE_BUCKETS: [(&str, usize); 5] = [
    ("\u{2264} 128 B", 128),
    ("\u{2264} 512 B", 512),
    ("\u{2264} 2 KiB", 2_048),
    ("\u{2264} 8 KiB", 8_192),
    ("> 8 KiB", usize::MAX),
];

/// Chunk payload size the resource transfer path uses (§14, §19.2).
pub const CHUNK_PAYLOAD_SIZE: usize = srui_sdk::CHUNK_PAYLOAD_SIZE;

/// How many latency and lag samples are retained. Bounded so a long-running demo cannot grow
/// without limit (§26 is about wire limits, but the same discipline applies to server memory).
const SAMPLE_WINDOW: usize = 256;

/// Facts read from the session after the gallery state lock is acquired and before a transaction
/// opens.
///
/// These non-revision accessors cannot be called from inside Session::transaction. The base and
/// committed revisions are filled from TransactionRevisions inside the actual commit critical
/// section, so concurrent built-in commits cannot make the rendered revision stale.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionFacts {
    pub attached: usize,
    pub base_revision: u64,
    pub committed_revision: u64,
    pub queue_capacity: usize,
    pub retained_bytes: Option<usize>,
}

impl SessionFacts {
    /// Reads non-revision session counters before the transaction opens.
    pub fn capture(session: &Session) -> Self {
        Self {
            attached: session.attached_count(),
            queue_capacity: session.outbound_queue_capacity(),
            retained_bytes: session.retained_client_state_bytes().ok(),
            ..Self::default()
        }
    }

    /// Associates the counters with revisions chosen atomically for the committing transaction.
    pub fn with_transaction(mut self, revisions: TransactionRevisions) -> Self {
        self.base_revision = revisions.base_revision;
        self.committed_revision = revisions.committed_revision;
        self
    }
}

/// Bounded traffic and latency accumulator.
#[derive(Debug)]
pub struct Metrics {
    transactions: u64,
    operations: u64,
    bytes: u64,
    buckets: [u64; SIZE_BUCKETS.len()],
    unframable: u64,
    events: u64,
    handling_micros: VecDeque<u64>,
    revision_lag: VecDeque<u64>,
    handling_summary: String,
    revision_lag_summary: String,
    summary_scratch: Vec<u64>,
    resource_bytes: u64,
    started: Instant,
    /// Last string written to each `(node, property)` pair, so a repeated render emits no
    /// operation (§23).
    rendered_text: HashMap<(NodeId, u32), String>,
    /// Last progress value written to each histogram bar, compared bitwise.
    rendered_value: HashMap<NodeId, u64>,
}

impl Clone for Metrics {
    fn clone(&self) -> Self {
        Self {
            transactions: self.transactions,
            operations: self.operations,
            bytes: self.bytes,
            buckets: self.buckets,
            unframable: self.unframable,
            events: self.events,
            handling_micros: self.handling_micros.clone(),
            revision_lag: self.revision_lag.clone(),
            handling_summary: self.handling_summary.clone(),
            revision_lag_summary: self.revision_lag_summary.clone(),
            summary_scratch: self.summary_scratch.clone(),
            resource_bytes: self.resource_bytes,
            started: self.started,
            rendered_text: self.rendered_text.clone(),
            rendered_value: self.rendered_value.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.transactions = source.transactions;
        self.operations = source.operations;
        self.bytes = source.bytes;
        self.buckets = source.buckets;
        self.unframable = source.unframable;
        self.events = source.events;
        self.handling_micros.clone_from(&source.handling_micros);
        self.revision_lag.clone_from(&source.revision_lag);
        self.handling_summary.clone_from(&source.handling_summary);
        self.revision_lag_summary
            .clone_from(&source.revision_lag_summary);
        self.summary_scratch.clone_from(&source.summary_scratch);
        self.resource_bytes = source.resource_bytes;
        self.started = source.started;
        self.rendered_text.clone_from(&source.rendered_text);
        self.rendered_value.clone_from(&source.rendered_value);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            transactions: 0,
            operations: 0,
            bytes: 0,
            buckets: [0; SIZE_BUCKETS.len()],
            unframable: 0,
            events: 0,
            handling_micros: VecDeque::new(),
            revision_lag: VecDeque::new(),
            handling_summary: "no samples yet (µs)".to_string(),
            revision_lag_summary: "no samples yet (rev)".to_string(),
            summary_scratch: Vec::with_capacity(SAMPLE_WINDOW),
            resource_bytes: 0,
            started: Instant::now(),
            rendered_text: HashMap::new(),
            rendered_value: HashMap::new(),
        }
    }
}

impl Metrics {
    /// Number of transactions observed so far.
    pub fn transactions(&self) -> u64 {
        self.transactions
    }

    /// Total framed bytes across every observed transaction.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Total operations across every observed transaction.
    pub fn operations(&self) -> u64 {
        self.operations
    }

    /// Number of client events observed so far.
    pub fn events(&self) -> u64 {
        self.events
    }

    /// Framed-size histogram, one counter per [`SIZE_BUCKETS`] entry.
    pub fn buckets(&self) -> [u64; SIZE_BUCKETS.len()] {
        self.buckets
    }

    /// Records the published resource payload so the panel can report its exact chunk count.
    pub fn observe_resource(&mut self, bytes: u64) {
        self.resource_bytes = bytes;
    }

    /// Number of 16 KiB chunks the published resource decomposes into (§14, §19.2).
    pub fn resource_chunks(&self) -> u64 {
        let chunk = CHUNK_PAYLOAD_SIZE as u64;
        self.resource_bytes.div_ceil(chunk)
    }

    /// Measures one committed operation list exactly as the connection would frame it (§26).
    ///
    /// A transaction that cannot be framed is counted separately rather than recorded as zero
    /// bytes: an oversize frame is precisely the condition these statistics exist to surface.
    pub fn observe_transaction(&mut self, base_revision: u64, operations: &[Operation]) {
        self.transactions = self.transactions.saturating_add(1);
        let operation_count = u64::try_from(operations.len()).unwrap_or(u64::MAX);
        self.operations = self.operations.saturating_add(operation_count);

        let transaction = Transaction::new(Revision::new(base_revision), operations.to_vec());
        let wire: srui_protocol::Transaction = (&transaction).into();
        let envelope = srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(wire)),
        };
        match srui_protocol::encode_framed(&envelope) {
            Ok(framed) => {
                let len = framed.len();
                let framed_bytes = u64::try_from(len).unwrap_or(u64::MAX);
                self.bytes = self.bytes.saturating_add(framed_bytes);
                let index = SIZE_BUCKETS
                    .iter()
                    .position(|(_, limit)| len <= *limit)
                    .unwrap_or(SIZE_BUCKETS.len() - 1);
                self.buckets[index] = self.buckets[index].saturating_add(1);
            }
            Err(error) => {
                self.unframable = self.unframable.saturating_add(1);
                tracing::warn!("revision {base_revision} could not be framed: {error}");
            }
        }
    }

    /// Records one client event: server handling time and how stale the client's view was.
    pub fn observe_event(&mut self, handling: Duration, revision_lag: u64) {
        self.events = self.events.saturating_add(1);
        let handling_micros = u64::try_from(handling.as_micros()).unwrap_or(u64::MAX);
        push_bounded(&mut self.handling_micros, handling_micros);
        push_bounded(&mut self.revision_lag, revision_lag);
        summarize_into(
            &self.handling_micros,
            "µs",
            &mut self.summary_scratch,
            &mut self.handling_summary,
        );
        summarize_into(
            &self.revision_lag,
            "rev",
            &mut self.summary_scratch,
            &mut self.revision_lag_summary,
        );
    }

    /// Writes the current statistics into the telemetry nodes, emitting operations only for the
    /// values that actually changed (§23).
    pub fn render(
        &mut self,
        ui: &mut UiTransaction,
        facts: &SessionFacts,
    ) -> Result<(), StoreError> {
        let retained = match facts.retained_bytes {
            Some(bytes) => format!("{bytes} B"),
            None => "unavailable".to_string(),
        };

        self.set_text(
            ui,
            ids::CONN_CLIENTS,
            format!("Attached clients: {}", facts.attached),
        )?;
        self.set_text(
            ui,
            ids::CONN_REVISION,
            format!(
                "Revision: {} (journal head {})",
                facts.committed_revision, facts.committed_revision
            ),
        )?;
        self.set_text(
            ui,
            ids::CONN_QUEUE,
            format!("Outbound queue capacity: {}", facts.queue_capacity),
        )?;
        self.set_text(
            ui,
            ids::CONN_RETAINED,
            format!("Retained client state: {retained}"),
        )?;

        let elapsed = self.started.elapsed().as_secs_f64().max(0.001);
        let mean_ops = if self.transactions == 0 {
            0.0
        } else {
            self.operations as f64 / self.transactions as f64
        };
        let unframable = if self.unframable == 0 {
            String::new()
        } else {
            format!(", {} unframable", self.unframable)
        };
        self.set_text(
            ui,
            ids::CONN_THROUGHPUT,
            format!(
                "Server\u{2192}client: {} txn, {} ops, {} B framed ({:.0} B/s, {mean_ops:.1} ops/txn){unframable}",
                self.transactions,
                self.operations,
                self.bytes,
                self.bytes as f64 / elapsed,
            ),
        )?;
        self.set_text(
            ui,
            ids::CONN_RESOURCE,
            format!(
                "Resource: {} B in {} chunks of {} B",
                self.resource_bytes,
                self.resource_chunks(),
                CHUNK_PAYLOAD_SIZE
            ),
        )?;
        self.set_text(
            ui,
            ids::CONN_LATENCY,
            format!(
                "Server-side handling ({} events): {}",
                self.events, self.handling_summary
            ),
        )?;
        self.set_text(
            ui,
            ids::CONN_LAG,
            format!("Client revision lag: {}", self.revision_lag_summary),
        )?;

        let total = self
            .buckets
            .iter()
            .fold(0_u64, |sum, count| sum.saturating_add(*count));
        for (index, node) in ids::CONN_HIST_BARS.iter().enumerate() {
            let count = self.buckets[index];
            let fraction = if total == 0 {
                0.0
            } else {
                count as f64 / total as f64
            };
            self.set_value(ui, *node, fraction)?;
            self.set_text_property(
                ui,
                *node,
                srui_sdk::VALUE_DESCRIPTION,
                format!("{} \u{b7} {count}", SIZE_BUCKETS[index].0),
            )?;
        }

        Ok(())
    }

    fn set_text(
        &mut self,
        ui: &mut UiTransaction,
        node: NodeId,
        text: String,
    ) -> Result<(), StoreError> {
        self.set_text_property(ui, node, srui_sdk::TEXT, text)
    }

    fn set_text_property(
        &mut self,
        ui: &mut UiTransaction,
        node: NodeId,
        property: srui_sdk::PropertyRef,
        text: String,
    ) -> Result<(), StoreError> {
        let key = (node, property.local_id);
        if self
            .rendered_text
            .get(&key)
            .is_some_and(|prev| *prev == text)
        {
            return Ok(());
        }
        ui.set(node, property, Value::String(text.clone()))?;
        self.rendered_text.insert(key, text);
        Ok(())
    }

    fn set_value(
        &mut self,
        ui: &mut UiTransaction,
        node: NodeId,
        value: f64,
    ) -> Result<(), StoreError> {
        let bits = value.to_bits();
        if self.rendered_value.get(&node) == Some(&bits) {
            return Ok(());
        }
        Progress::set_value_for(ui, node, value)?;
        self.rendered_value.insert(node, bits);
        Ok(())
    }
}

fn push_bounded(samples: &mut VecDeque<u64>, sample: u64) {
    if samples.len() == SAMPLE_WINDOW {
        samples.pop_front();
    }
    samples.push_back(sample);
}

/// Updates a cached min / p50 / p95 / max summary after a new sample arrives.
fn summarize_into(
    samples: &VecDeque<u64>,
    unit: &str,
    scratch: &mut Vec<u64>,
    output: &mut String,
) {
    scratch.clear();
    scratch.extend(samples.iter().copied());
    scratch.sort_unstable();
    output.clear();

    if scratch.is_empty() {
        write!(output, "no samples yet ({unit})").expect("writing to String cannot fail");
        return;
    }

    let pick = |q: f64| -> u64 {
        let index = ((scratch.len() - 1) as f64 * q).round() as usize;
        scratch[index]
    };
    write!(
        output,
        "min {} · p50 {} · p95 {} · max {} {unit}",
        scratch[0],
        pick(0.50),
        pick(0.95),
        scratch[scratch.len() - 1]
    )
    .expect("writing to String cannot fail");
    scratch.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_summary_caches_change_only_when_samples_change() {
        let mut metrics = Metrics::default();
        assert_eq!(metrics.handling_summary, "no samples yet (µs)");
        assert_eq!(metrics.revision_lag_summary, "no samples yet (rev)");

        metrics.observe_event(Duration::from_micros(10), 3);
        let first_handling = metrics.handling_summary.clone();
        let first_lag = metrics.revision_lag_summary.clone();
        assert_eq!(first_handling, "min 10 · p50 10 · p95 10 · max 10 µs");
        assert_eq!(first_lag, "min 3 · p50 3 · p95 3 · max 3 rev");

        metrics.observe_transaction(0, &[]);
        assert_eq!(metrics.handling_summary, first_handling);
        assert_eq!(metrics.revision_lag_summary, first_lag);

        metrics.observe_event(Duration::from_micros(30), 7);
        assert_eq!(
            metrics.handling_summary,
            "min 10 · p50 30 · p95 30 · max 30 µs"
        );
        assert_eq!(
            metrics.revision_lag_summary,
            "min 3 · p50 7 · p95 7 · max 7 rev"
        );
        assert!(metrics.summary_scratch.is_empty());
    }
}
