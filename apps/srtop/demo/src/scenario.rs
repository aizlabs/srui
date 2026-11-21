//! Repeatable latency scenarios against a running srtop (PX-025; D1 §31.4).
//!
//! Each scenario is composed from what already exists: one of srtop's own
//! deterministic fixtures, a [`DelayRelay`] in the transport path and a
//! [`Replica`] client. None adds protocol, retry or reconnect logic: the
//! interruption is resumed with the runtime's own `CLIENT_RESUME` (§18), and
//! the collector failure is the scripted fixture's failed scan (PX-004).
//!
//! * [`remote_results`]: server-dependent results — the filter's and the
//!   search's committed status after a `TEXT_EDIT` — at one RTT. A result
//!   needs the edit to reach srtop and the commit to come back, so it cannot
//!   arrive sooner than one RTT.
//! * [`transport_interruption`]: every connection is cut while srtop keeps
//!   committing; the client resumes through the same relay.
//! * [`collector_failure`]: the scripted source's failed scan reaches a client
//!   through the relay as a failure — rows retained, the status and freshness
//!   line saying why, nothing turned into zero — and recovery converges.

use crate::{
    relay::{DelayRelay, Traffic},
    wire::{Decision, Replica, Result},
};
use srui_process_explorer::{
    source::SEQUENCE_STATUS_TEXT, summary::FRESHNESS_TEXT, FILTER_INPUT, FILTER_STATUS,
    SEARCH_INPUT, SEARCH_STATUS, STATUS,
};
use srui_semantic_tree::NodeId;
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// The RTTs D1 §31.4 names.
pub const RTTS_MS: [u64; 4] = [0, 100, 300, 600];

const WAIT: Duration = Duration::from_secs(20);

/// A client instance identity no earlier attach in this process used: a
/// reused one would continue that client's event sequence (§18.2).
fn instance(scenario: &str, rtt: Duration) -> Vec<u8> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "px025-{scenario}-{}ms-{}-{}",
        rtt.as_millis(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
    .into_bytes()
}

/// The median of `samples` (the upper one of an even count).
pub fn median(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort();
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn json_list(samples: &[Duration]) -> String {
    let items: Vec<String> = samples
        .iter()
        .map(|sample| format!("{:.1}", ms(*sample)))
        .collect();
    format!("[{}]", items.join(", "))
}

fn json_text(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

/// Server-dependent result latency at one RTT.
#[derive(Debug, Clone)]
pub struct RemoteResultReport {
    pub rtt: Duration,
    /// From `CLIENT_HELLO` sent to the snapshot applied.
    pub attach: Duration,
    /// From each filter `TEXT_EDIT` sent to its committed status applied.
    pub filter: Vec<Duration>,
    /// The same for the search field.
    pub search: Vec<Duration>,
    /// Everything the relay carried for this client.
    pub traffic: Traffic,
}

impl RemoteResultReport {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"rtt_ms\": {}, \"attach_ms\": {:.1}, \"filter_ms\": {}, \"filter_p50_ms\": {:.1}, \"search_ms\": {}, \"search_p50_ms\": {:.1}, \"bytes_up\": {}, \"bytes_down\": {}}}",
            self.rtt.as_millis(),
            ms(self.attach),
            json_list(&self.filter),
            ms(median(&self.filter)),
            json_list(&self.search),
            ms(median(&self.search)),
            self.traffic.up,
            self.traffic.down
        )
    }
}

/// Terms that each change the committed status of the field they are typed
/// into, whatever the previous one was.
const TERMS: [&str; 4] = ["worker", "w", "wor", "work"];

/// Types a term into `input` as one `TEXT_EDIT` and waits for `status` to
/// commit a new text naming it; returns that round trip.
///
/// The term is the first of [`TERMS`], from `start`, that the status does not
/// already name: srtop's filter and search state belongs to the session, so a
/// commit still on its way may re-state an earlier client's term, and only a
/// term the status has not named yet proves the result is this edit's.
fn round_trip(
    replica: &mut Replica,
    input: NodeId,
    status: NodeId,
    start: usize,
) -> Result<Duration> {
    let before = replica.text(status).unwrap_or_default();
    let quoted = (0..TERMS.len())
        .map(|offset| format!("\"{}\"", TERMS[(start + offset) % TERMS.len()]))
        .find(|quoted| !before.contains(quoted.as_str()))
        .ok_or("every term is already named")?;
    let term = quoted.trim_matches('"').to_string();
    let term = term.as_str();
    let sent = Instant::now();
    replica.send_text_edit(input, term)?;
    replica.until(
        &format!("the committed result for {quoted}"),
        WAIT,
        |replica| {
            replica
                .text(status)
                .is_some_and(|now| now != before && now.contains(&quoted))
        },
    )?;
    Ok(sent.elapsed())
}

/// Attaches through a fresh relay at `rtt` in front of `upstream` (an srtop
/// serving `--fake-repeated-names`) and times `trials` filter and search
/// results.
pub fn remote_results(
    upstream: &Path,
    relay_socket: &Path,
    rtt: Duration,
    trials: usize,
) -> Result<RemoteResultReport> {
    let relay =
        DelayRelay::start(relay_socket, upstream, rtt).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mut replica = Replica::hello(relay_socket, &instance("remote", rtt))?;
    let attach = started.elapsed();
    let mut filter = Vec::with_capacity(trials);
    for trial in 0..trials {
        filter.push(round_trip(
            &mut replica,
            FILTER_INPUT,
            FILTER_STATUS,
            trial,
        )?);
    }
    // Every row back, so the search runs over the whole table.
    let before = replica.text(FILTER_STATUS).unwrap_or_default();
    replica.send_text_edit(FILTER_INPUT, "")?;
    replica.until("the cleared filter", WAIT, |replica| {
        replica
            .text(FILTER_STATUS)
            .is_some_and(|now| now != before && now.contains(": none"))
    })?;
    let mut search = Vec::with_capacity(trials);
    for trial in 0..trials {
        search.push(round_trip(
            &mut replica,
            SEARCH_INPUT,
            SEARCH_STATUS,
            trial,
        )?);
    }
    let traffic = relay.traffic();
    Ok(RemoteResultReport {
        rtt,
        attach,
        filter,
        search,
        traffic,
    })
}

/// A transport interruption at one RTT.
#[derive(Debug, Clone)]
pub struct InterruptionReport {
    pub rtt: Duration,
    /// Connections the relay cut.
    pub cut: usize,
    /// The replica's revision when it noticed the cut.
    pub revision_at_cut: u64,
    /// The runtime's continuity decision on resume.
    pub decision: Decision,
    /// From `CLIENT_RESUME` sent to that decision.
    pub resume: Duration,
    /// The revision the replica caught up to after resuming.
    pub revision_after: u64,
}

impl InterruptionReport {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"rtt_ms\": {}, \"cut_connections\": {}, \"revision_at_cut\": {}, \"decision\": {}, \"resume_ms\": {:.1}, \"revision_after\": {}}}",
            self.rtt.as_millis(),
            self.cut,
            self.revision_at_cut,
            json_text(&format!("{:?}", self.decision)),
            ms(self.resume),
            self.revision_after
        )
    }
}

/// Attaches through a relay at `rtt` in front of `upstream` (an srtop
/// serving `--fake-sequence`, which commits on most ticks), cuts the
/// transport, lets srtop commit on while the client is detached, and resumes
/// through the same relay.
pub fn transport_interruption(
    upstream: &Path,
    relay_socket: &Path,
    rtt: Duration,
    detached: Duration,
) -> Result<InterruptionReport> {
    let relay =
        DelayRelay::start(relay_socket, upstream, rtt).map_err(|error| error.to_string())?;
    let mut replica = Replica::hello(relay_socket, &instance("interruption", rtt))?;
    let attached = replica.revision();
    replica.until("two commits", WAIT, |replica| {
        replica.revision() >= attached + 2
    })?;
    let cut = relay.cut();
    // The client sees the transport end: it is told nothing else.
    let noticed = loop {
        if replica.next_message().is_err() {
            break replica.revision();
        }
    };
    std::thread::sleep(detached);
    let sent = Instant::now();
    let decision = replica.resume(relay_socket)?;
    let resume = sent.elapsed();
    replica.until("the replay", WAIT, |replica| replica.revision() > noticed)?;
    Ok(InterruptionReport {
        rtt,
        cut,
        revision_at_cut: noticed,
        decision,
        resume,
        revision_after: replica.revision(),
    })
}

/// A collector failure, as a client through the relay sees it.
#[derive(Debug, Clone)]
pub struct CollectorFailureReport {
    pub rtt: Duration,
    /// The rows of the last successful scan before the failure.
    pub rows_before: Vec<Vec<String>>,
    /// The rows while the collector is failing.
    pub rows_during: Vec<Vec<String>>,
    pub status_during: String,
    pub freshness_during: String,
    /// The rows once a scan succeeds again.
    pub rows_after: Vec<Vec<String>>,
    pub status_after: String,
}

impl CollectorFailureReport {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"rtt_ms\": {}, \"rows_before\": {}, \"rows_during\": {}, \"rows_after\": {}, \"status_during\": {}, \"freshness_during\": {}, \"status_after\": {}}}",
            self.rtt.as_millis(),
            self.rows_before.len(),
            self.rows_during.len(),
            self.rows_after.len(),
            json_text(&self.status_during),
            json_text(&self.freshness_during),
            json_text(&self.status_after)
        )
    }
}

/// What srtop's status says during the scripted fixture's failed scan.
pub const FAILED_SCAN: &str = "incomplete scan · process list unavailable";
/// What the freshness line says then.
pub const COLLECTOR_ERROR: &str = "Collector error: could not list processes";

/// Attaches through a relay at `rtt` in front of `upstream` (an srtop
/// serving `--fake-sequence`) and follows one failed scan and its recovery.
pub fn collector_failure(
    upstream: &Path,
    relay_socket: &Path,
    rtt: Duration,
) -> Result<CollectorFailureReport> {
    let _relay =
        DelayRelay::start(relay_socket, upstream, rtt).map_err(|error| error.to_string())?;
    let mut replica = Replica::hello(relay_socket, &instance("collector", rtt))?;
    let healthy = |replica: &Replica| {
        replica.text(STATUS).as_deref() == Some(SEQUENCE_STATUS_TEXT)
            && replica
                .rows()
                .iter()
                .any(|row| row.iter().any(|cell| cell == "builder"))
    };
    replica.until("a successful scan with the settled rows", WAIT, healthy)?;
    // The rows of the last healthy state before the failure was applied.
    let mut rows_before = replica.rows();
    let deadline = Instant::now() + WAIT;
    while !replica
        .text(STATUS)
        .is_some_and(|status| status.contains(FAILED_SCAN))
    {
        if Instant::now() >= deadline {
            return Err("timed out waiting for the failed scan".into());
        }
        if healthy(&replica) {
            rows_before = replica.rows();
        }
        replica.next_message()?;
    }
    let rows_during = replica.rows();
    let status_during = replica.text(STATUS).unwrap_or_default();
    let freshness_during = replica.text(FRESHNESS_TEXT).unwrap_or_default();
    replica.until("recovery", WAIT, |replica| {
        replica.text(STATUS).as_deref() == Some(SEQUENCE_STATUS_TEXT)
    })?;
    Ok(CollectorFailureReport {
        rtt,
        rows_before,
        rows_during,
        status_during,
        freshness_during,
        rows_after: replica.rows(),
        status_after: replica.text(STATUS).unwrap_or_default(),
    })
}
