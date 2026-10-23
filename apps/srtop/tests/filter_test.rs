//! PX-010 acceptance: responsive command filtering for srtop (D1 §6.2, §7.6, §8, §22.6).
//!
//! * Grammar and syntax: fixed-string matching, Unicode case folding, OR splitting on `|`,
//!   512-byte query cap, 16-term limit, and inert text sanitization (zero regex/shell execution).
//! * Two-phase staging and monotonic generations: older staged filters never overwrite newer ones.
//! * ItemId stability: processes filtered out retain their ItemId on the server so un-filtering
//!   or re-matching restores identical item IDs, maintaining native selection.
//! * Composition with sorting: filtered views maintain the active SortOrder.
//! * Event integration: client TEXT_EDIT events on FILTER_INPUT automatically update the model
//!   and filter status.

use srui_process_explorer::catalogue::{ProcessField, SortDirection};
use srui_process_explorer::filter::{FilterQuery, MAX_FILTER_QUERY_BYTES, MAX_FILTER_TERMS};
use srui_process_explorer::sort::SortOrder;
use srui_process_explorer::source::*;
use srui_process_explorer::{
    install_filter_handlers, install_sort_handlers, start_from_source, FILTER_INPUT, FILTER_STATUS,
    MODEL,
};
use srui_protocol::Event as WireEvent;
use srui_sdk::{ItemId, NodeId, Value, TEXT, VALUE};
use srui_semantic_tree::{EditSeq, Event as DomainEvent};
use srui_sessiond::{EventOutcome, Session};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SOURCE: &str = "filter-fixture-v1";
const CLIENT: &[u8] = b"filter-client-instance";

fn key(pid: Option<u32>, token: &str) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("fixture-host".into())),
        boot: Observed::Known(BootId("fixture-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
        pid: pid.map_or(
            Observed::Missing(MissingReason::Unavailable),
            Observed::Known,
        ),
        creation: CreationToken::Opaque(token.into()),
    }
}

fn cpu(percent: u64) -> CpuUsage {
    CpuUsage::Measured(CpuInterval {
        ticks: percent,
        ticks_per_second: 100,
        elapsed: Duration::from_secs(1),
    })
}

fn record(
    pid: Option<u32>,
    token: &str,
    name: &str,
    cmd: Option<&[u8]>,
    resident: u64,
    cpu_share: u64,
) -> ProcessRecord {
    let mut display_name = DisplayName::from(name);
    if let Some(cmd_bytes) = cmd {
        display_name = display_name.with_command(cmd_bytes);
    }
    ProcessRecord {
        key: key(pid, token),
        display_name,
        resident: Observed::Known(resident),
        cpu: cpu(cpu_share),
    }
}

struct FixtureSource(Vec<ProcessRecord>);

impl ProcessSource for FixtureSource {
    fn status_text(&self) -> &str {
        "fixture source ok"
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            sampled_at: SnapshotTime(std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            source: SourceId(SOURCE.into()),
            records: self.0.clone(),
            vanished: 0,
            capped: CappedRecords::none(),
            system: SystemSample::not_provided(),
            completeness: Completeness::Complete,
        }
    }
}

fn sample_processes() -> Vec<ProcessRecord> {
    vec![
        record(
            Some(1),
            "init",
            "systemd",
            Some(b"/sbin/init splash"),
            1024,
            1,
        ),
        record(Some(10), "zsh", "zsh", Some(b"-zsh"), 2048, 5),
        record(
            Some(100),
            "worker-1",
            "worker",
            Some(b"/usr/bin/worker --job=ingest"),
            4096,
            20,
        ),
        record(
            Some(101),
            "worker-2",
            "worker",
            Some(b"/usr/bin/worker --job=cleanup"),
            4096,
            15,
        ),
        record(
            Some(200),
            "nginx",
            "nginx",
            Some(b"nginx: master process /usr/sbin/nginx"),
            8192,
            2,
        ),
        record(
            Some(300),
            "python",
            "python3",
            Some(b"python3 -m http.server 8080"),
            16384,
            10,
        ),
        record(
            Some(400),
            "rust",
            "cargo",
            Some(b"cargo test --filter-test"),
            32768,
            50,
        ),
    ]
}

fn text_edit(event_seq: u64, event_id: &str, text: &str, seq: u64) -> WireEvent {
    DomainEvent::text_edit(
        event_seq,
        event_id,
        0u64,
        FILTER_INPUT.get(),
        text,
        EditSeq::new(seq).expect("positive edit_seq"),
    )
    .with_client_instance_id(CLIENT.to_vec())
    .to_wire()
}

fn store_status(session: &Session, node_id: NodeId) -> String {
    session.with_store(|store| {
        store
            .get_node(node_id)
            .and_then(|node| match node.properties.get(&TEXT) {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default()
    })
}

fn store_input_value(session: &Session) -> String {
    session.with_store(|store| {
        store
            .get_node(FILTER_INPUT)
            .and_then(|node| match node.properties.get(&VALUE) {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default()
    })
}

fn rows(session: &Session) -> Vec<(ItemId, Vec<String>)> {
    session.with_store(|store| {
        let model = store.get_model(MODEL).expect("the collection model exists");
        assert_eq!(model.item_count, model.items.len() as u64);
        model
            .items
            .values()
            .map(|item| {
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let cells = cells
                    .iter()
                    .map(|cell| match cell {
                        Value::String(text) => text.clone(),
                        Value::UnsignedInt(n) => n.to_string(),
                        other => other.to_string(),
                    })
                    .collect();
                (item.item_id, cells)
            })
            .collect()
    })
}

fn pids(session: &Session) -> Vec<String> {
    rows(session)
        .into_iter()
        .map(|(_, cells)| cells[0].clone())
        .collect()
}

fn names(session: &Session) -> Vec<String> {
    rows(session)
        .into_iter()
        .map(|(_, cells)| cells[1].clone())
        .collect()
}

#[test]
fn query_grammar_term_limits_and_sanitization() {
    // Basic query
    let q = FilterQuery::parse("worker");
    assert_eq!(q.terms().len(), 1);
    assert_eq!(q.terms()[0], "worker");

    // Case-insensitivity & folding
    let q_upper = FilterQuery::parse("WORKER");
    assert_eq!(q_upper.terms()[0], "worker");

    // Multiple terms with OR splitting on '|'
    let q_or = FilterQuery::parse("nginx | python | zsh");
    assert_eq!(q_or.terms(), &["nginx", "python", "zsh"]);

    // Whitespace trimming in terms
    let q_spaces = FilterQuery::parse("   worker   |   nginx   ");
    assert_eq!(q_spaces.terms(), &["worker", "nginx"]);

    // Empty terms are dropped
    let q_empty_terms = FilterQuery::parse("worker | | | nginx | ");
    assert_eq!(q_empty_terms.terms(), &["worker", "nginx"]);

    // Term limit: max 16 terms
    let many_terms = (0..25)
        .map(|i| format!("term{i}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let q_capped = FilterQuery::parse(&many_terms);
    assert_eq!(q_capped.terms().len(), MAX_FILTER_TERMS);
    assert_eq!(q_capped.terms()[0], "term0");
    assert_eq!(q_capped.terms()[15], "term15");

    // Query length limit: max 512 bytes
    let long_raw = "a".repeat(MAX_FILTER_QUERY_BYTES + 100);
    let q_long = FilterQuery::parse(&long_raw);
    assert!(q_long.raw().len() <= MAX_FILTER_QUERY_BYTES);

    // Inert text: regex and control characters are inert literals
    let q_regex = FilterQuery::parse(".*[a-z]+$");
    assert_eq!(q_regex.terms()[0], ".*[a-z]+$");
    let rec = record(Some(1), "t", ".*[a-z]+$", None, 100, 1);
    assert!(q_regex.matches_record(&rec));
    let non_matching = record(Some(2), "t", "normal", None, 100, 1);
    assert!(!q_regex.matches_record(&non_matching));
}

#[test]
fn filter_stages_and_commits_with_status_updates() {
    let session = Session::mint();
    let mut source = FixtureSource(sample_processes());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();

    assert_eq!(view.row_count(), 7);
    assert_eq!(view.unfiltered_row_count(), 7);
    assert_eq!(view.committed_filter().raw(), "");

    // Stage filter
    let gen = view.stage_filter(&session, "worker").unwrap();
    assert_eq!(gen.get(), 1);
    assert!(store_status(&session, FILTER_STATUS).contains("Filter pending (gen 1)"));

    // Commit filter
    let outcome = view.commit_filter(&session, gen).unwrap();
    assert_eq!(outcome.deleted, 5);
    assert_eq!(view.row_count(), 2);
    assert_eq!(view.unfiltered_row_count(), 7);
    assert_eq!(view.committed_filter().raw(), "worker");
    assert_eq!(
        store_status(&session, FILTER_STATUS),
        "Committed filter (gen 1): \"worker\" · 2 of 7 rows matched"
    );
    assert_eq!(names(&session), vec!["worker", "worker"]);

    // Clearing filter
    let gen2 = view.stage_filter(&session, "").unwrap();
    assert_eq!(gen2.get(), 2);
    view.commit_filter(&session, gen2).unwrap();
    assert_eq!(view.row_count(), 7);
    assert_eq!(
        store_status(&session, FILTER_STATUS),
        "Committed filter (gen 2): none · showing all 7 rows"
    );
}

#[test]
fn stale_generations_are_rejected_and_do_not_overwrite_newer_filters() {
    let session = Session::mint();
    let mut source = FixtureSource(sample_processes());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();

    // Stage filter 1
    let gen1 = view.stage_filter(&session, "worker").unwrap();
    // Stage filter 2
    let gen2 = view.stage_filter(&session, "nginx").unwrap();
    assert_eq!(gen1.get(), 1);
    assert_eq!(gen2.get(), 2);

    // Commit newer generation 2 first
    view.commit_filter(&session, gen2).unwrap();
    assert_eq!(view.committed_filter().raw(), "nginx");
    assert_eq!(view.row_count(), 1);
    assert_eq!(names(&session), vec!["nginx"]);

    // Attempting to commit older generation 1 is refused and rejected
    let outcome1 = view.commit_filter(&session, gen1).unwrap();
    assert_eq!(outcome1.inserted, 0);
    assert_eq!(outcome1.deleted, 0);
    // View still has "nginx" filter, not "worker"
    assert_eq!(view.committed_filter().raw(), "nginx");
    assert_eq!(view.row_count(), 1);
    assert_eq!(names(&session), vec!["nginx"]);
}

#[test]
fn item_ids_remain_stable_across_filter_and_unfilter() {
    let session = Session::mint();
    let mut source = FixtureSource(sample_processes());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();

    let initial_rows = rows(&session);
    assert_eq!(initial_rows.len(), 7);

    let id_by_pid: HashMap<String, ItemId> = initial_rows
        .iter()
        .map(|(id, cells)| (cells[0].clone(), *id))
        .collect();

    // Filter down to "worker" (PIDs 100 and 101)
    view.set_filter(&session, "worker").unwrap();
    let filtered_rows = rows(&session);
    assert_eq!(filtered_rows.len(), 2);
    assert_eq!(filtered_rows[0].0, id_by_pid["100"]);
    assert_eq!(filtered_rows[1].0, id_by_pid["101"]);

    // Clear filter
    view.set_filter(&session, "").unwrap();
    let restored_rows = rows(&session);
    assert_eq!(restored_rows.len(), 7);
    for (id, cells) in &restored_rows {
        assert_eq!(
            *id, id_by_pid[&cells[0]],
            "PID {} retained its item ID",
            cells[0]
        );
    }
}

#[test]
fn matching_against_command_line_arguments() {
    let session = Session::mint();
    let mut source = FixtureSource(sample_processes());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();

    // Query matches command argument "--job=ingest" (worker PID 100)
    view.set_filter(&session, "ingest").unwrap();
    assert_eq!(view.row_count(), 1);
    assert_eq!(pids(&session), vec!["100"]);

    // Query matches another argument "8080" (python3 PID 300)
    view.set_filter(&session, "8080").unwrap();
    assert_eq!(view.row_count(), 1);
    assert_eq!(pids(&session), vec!["300"]);

    // Query with OR matching both command args
    view.set_filter(&session, "ingest | 8080").unwrap();
    assert_eq!(view.row_count(), 2);
    assert_eq!(pids(&session), vec!["100", "300"]);
}

#[test]
fn filter_composes_with_sort_orders() {
    let session = Session::mint();
    let mut source = FixtureSource(sample_processes());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();

    // Sort by CPU descending
    let cpu_desc = SortOrder {
        field: ProcessField::Cpu,
        direction: SortDirection::Descending,
    };
    view.set_sort(&session, cpu_desc).unwrap();
    // Highest CPU is cargo (50%), PID 400
    assert_eq!(pids(&session)[0], "400");

    // Filter by "worker | cargo"
    view.set_filter(&session, "worker | cargo").unwrap();
    assert_eq!(view.row_count(), 3);
    // Highest CPU first: cargo (50%, PID 400), worker-1 (20%, PID 100), worker-2 (15%, PID 101)
    assert_eq!(pids(&session), vec!["400", "100", "101"]);

    // Toggle sort to CPU ascending while filtered
    let cpu_asc = SortOrder {
        field: ProcessField::Cpu,
        direction: SortDirection::Ascending,
    };
    view.set_sort(&session, cpu_asc).unwrap();
    assert_eq!(view.row_count(), 3);
    // Lowest CPU first among matched: worker-2 (15%, PID 101), worker-1 (20%, PID 100), cargo (50%, PID 400)
    assert_eq!(pids(&session), vec!["101", "100", "400"]);
}

#[test]
fn text_edit_event_dispatches_through_session() {
    let session = Arc::new(Session::mint());
    let mut source = FixtureSource(sample_processes());
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    let shared_view = Arc::new(Mutex::new(view));

    install_filter_handlers(&session, &shared_view);
    install_sort_handlers(&session, &shared_view);

    // Send a client TEXT_EDIT event to FILTER_INPUT
    let edit1 = text_edit(1, "evt1", "nginx", 1);
    let outcome = session.process_event(&edit1).expect("event admitted");
    assert!(matches!(outcome, EventOutcome::Processed { .. }));

    // Model and store should be filtered to "nginx"
    assert_eq!(store_input_value(&session), "nginx");
    assert_eq!(rows(&session).len(), 1);
    assert_eq!(pids(&session), vec!["200"]);
    assert_eq!(
        store_status(&session, FILTER_STATUS),
        "Committed filter (gen 1): \"nginx\" · 1 of 7 rows matched"
    );

    // Send a second TEXT_EDIT event clearing the filter
    let edit2 = text_edit(2, "evt2", "", 2);
    let outcome2 = session.process_event(&edit2).expect("event admitted");
    assert!(matches!(outcome2, EventOutcome::Processed { .. }));

    assert_eq!(store_input_value(&session), "");
    assert_eq!(rows(&session).len(), 7);
    assert_eq!(
        store_status(&session, FILTER_STATUS),
        "Committed filter (gen 2): none · showing all 7 rows"
    );
}
