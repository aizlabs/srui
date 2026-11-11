//! PX-014 acceptance: the selected process's command line and executable
//! context (D1 §§7.2–7.3, 22.4–22.6, 26–27; D2 T29, T32).
//!
//! * The inspector's command-line view is one standard read-only `RichText`
//!   under its four lines. It shows the argument vector one argument per line,
//!   with every quote, newline, escape byte, shell metacharacter and
//!   bidirectional override as inert text, then the executable and working
//!   directory as server text.
//! * It is read lazily, with the details, for the current target only, and it
//!   follows the target's identity like every inspector line: a refused,
//!   ended, filtered or denied target has its own text, never another
//!   process's arguments.
//! * A secret-bearing argument is shown where it belongs and nowhere else: no
//!   standard output, no standard error, no `Debug` rendering. The real
//!   handlers and the real detail worker are run in a child process whose
//!   output is captured whole.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::command::MAX_COMMAND_TEXT_BYTES;
use srui_process_explorer::inspector::DetailReadsIdle;
use srui_process_explorer::refresh::{lock_view, SharedView};
use srui_process_explorer::source::{FakeProcessSource, ProcessSource, FIXTURE_SECRET_ARGUMENT};
use srui_process_explorer::{
    install_column_handlers, install_filter_handlers, install_inspector, install_owner_handlers,
    install_search_handlers, install_sort_handlers, start_from_source, INSPECTOR_COMMAND, MODEL,
    TABLE,
};
use srui_sdk::{ItemId, NodeId, Operation, PropertyRef, TypeRef, Value, READ_ONLY, TEXT};
use srui_semantic_tree::Event as DomainEvent;
use srui_sessiond::{EventOutcome, Session};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CLIENT: &[u8] = b"command-line-client-instance";
const WAIT: Duration = Duration::from_secs(10);
/// The part of [`FIXTURE_SECRET_ARGUMENT`] no output may contain.
const SECRET: &str = "hunter2-SECRET";

/// The fake fixture's first worker (PID 4101), as its view shows it.
const WORKER_A: &str =
    "Command line (argv on the server, shown as text, never run): 8 arguments, 182 bytes
argv[0] · 33 bytes · /usr/libexec/srtop-fixture/worker
argv[1] · 13 bytes · --role=worker
argv[2] · 21 bytes · --label=it's \"quoted\"
argv[3] · 17 bytes · line one\\nline two
argv[4] · 12 bytes · \\x1b[31mred\\x1b[0m
argv[5] · 43 bytes · ; rm -rf / # $(touch /tmp/px014) `id` | tee
argv[6] · 10 bytes · \\u{202e}gpj.exe
argv[7] · 25 bytes · --password=hunter2-SECRET
Executable on the server: /usr/libexec/srtop-fixture/worker
Working directory on the server: /home/alice/work";

/// The second worker (PID 4102), another account's process.
const WORKER_B: &str =
    "Command line (argv on the server, shown as text, never run): 2 arguments, 21 bytes
argv[0] · 6 bytes · worker
argv[1] · 13 bytes · --role=worker
Executable on the server: permission denied
Working directory on the server: permission denied";

/// The fake source wired exactly as `srtop --fake-source` wires it.
struct Explorer {
    session: Arc<Session>,
    view: SharedView,
    source: FakeProcessSource,
    idle: DetailReadsIdle,
    seq: std::cell::Cell<u64>,
}

impl Explorer {
    fn start() -> Self {
        let session = Arc::new(Session::mint());
        let mut source = FakeProcessSource;
        let (view, _) = start_from_source(&session, &mut source).unwrap();
        let view: SharedView = Arc::new(Mutex::new(view));
        install_sort_handlers(&session, &view);
        install_filter_handlers(&session, &view);
        install_column_handlers(&session, &view);
        install_search_handlers(&session, &view);
        install_owner_handlers(&session, &view);
        let idle = install_inspector(&session, &view, source.details())
            .expect("the fake source provides details");
        Self {
            session,
            view,
            source,
            idle,
            seq: std::cell::Cell::new(0),
        }
    }

    fn settle(&self) {
        assert!(self.idle.wait_idle(WAIT), "the detail worker settled");
    }

    fn item_of(&self, pid: u64) -> ItemId {
        self.session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .find(|item| {
                    matches!(&item.value, Value::List(cells)
                        if cells[ProcessField::Pid.index()] == Value::UnsignedInt(pid))
                })
                .map(|item| item.item_id)
                .expect("the fixture publishes this PID")
        })
    }

    /// The item of the fixture's helper, whose PID is unavailable.
    fn helper(&self) -> ItemId {
        self.session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .find(|item| {
                    matches!(&item.value, Value::List(cells)
                        if cells[ProcessField::Name.index()] == Value::String("helper".into()))
                })
                .map(|item| item.item_id)
                .expect("the fixture publishes its helper")
        })
    }

    fn select_with(&self, arguments: Vec<(PropertyRef, Value)>) {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        let event = DomainEvent::new(
            None,
            seq,
            format!("command-select-{seq}"),
            self.session.current_revision(),
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        let outcome = self.session.process_event(&event).expect("event admitted");
        assert!(matches!(outcome, EventOutcome::Processed { .. }));
    }

    fn select(&self, item: ItemId) {
        self.select_with(vec![(PropertyRef::VALUE, Value::ItemId(item))]);
    }

    fn command(&self) -> String {
        let held = self.session.with_store(|store| {
            match store
                .get_node(INSPECTOR_COMMAND)
                .and_then(|node| node.get_property(TEXT))
            {
                Some(Value::String(text)) => text.clone(),
                _ => panic!("the command line has no text"),
            }
        });
        assert_eq!(
            held,
            lock_view(&self.view).inspector_view().command,
            "the view records what committed"
        );
        held
    }

    /// Every command-line text committed since `revision`.
    fn commands_since(&self, revision: u64) -> Vec<String> {
        self.session
            .collect_replayed_transactions(revision)
            .unwrap()
            .into_iter()
            .flat_map(|transaction| transaction.operations)
            .map(|op| Operation::try_from(op).unwrap())
            .filter_map(|op| match op {
                Operation::SetProperty {
                    id,
                    value: Value::String(text),
                    ..
                } if id == INSPECTOR_COMMAND => Some(text),
                _ => None,
            })
            .collect()
    }
}

/// The command line is a standard read-only `RichText` (selectable text in
/// the generic client, §§7.2, 22.4), and it shows the selected process's
/// argument vector as inert lines, its paths as server text, and a defined
/// state for every target that has no arguments to show.
#[test]
fn the_selected_processs_arguments_show_as_inert_lines_and_every_other_target_has_its_own_state() {
    let explorer = Explorer::start();
    explorer.session.with_store(|store| {
        let node = store.get_node(INSPECTOR_COMMAND).unwrap();
        assert_eq!(node.node_type, TypeRef::RICHTEXT);
        assert_eq!(node.get_property(READ_ONLY), Some(&Value::Bool(true)));
        // Plain text and nothing else: no action, no resource, no link.
        for property in node.properties.keys() {
            assert!(
                [
                    PropertyRef::TEXT,
                    PropertyRef::LABEL,
                    PropertyRef::ACCESSIBLE_DESCRIPTION,
                    PropertyRef::READ_ONLY
                ]
                .contains(property),
                "the command line carries {property:?}"
            );
        }
    });
    assert_eq!(explorer.command(), "Command line: no process selected");

    explorer.select(explorer.item_of(4101));
    explorer.settle();
    assert_eq!(explorer.command(), WORKER_A);
    assert!(explorer.command().len() <= MAX_COMMAND_TEXT_BYTES);
    // Every line is one argument or one path: an argument's own newline, or
    // a line or paragraph separator, can neither end its line nor start one.
    assert_eq!(explorer.command().lines().count(), 11);
    assert!(!explorer
        .command()
        .chars()
        .any(|c| (c.is_control() && c != '\n') || srui_process_explorer::command::is_hidden(c)));

    let revision = explorer.session.current_revision();
    explorer.select(explorer.item_of(4102));
    explorer.settle();
    assert_eq!(explorer.command(), WORKER_B);
    assert!(
        explorer
            .commands_since(revision)
            .iter()
            .all(|text| !text.contains(SECRET) && !text.contains("/home/alice")),
        "nothing of the earlier target is published after the later one is selected"
    );

    explorer.select(explorer.helper());
    explorer.settle();
    assert_eq!(explorer.command(), "Command line: permission denied");

    // A filter that hides the target shows it as not shown, and none of its
    // arguments, until the filter is cleared.
    explorer.select(explorer.item_of(4101));
    explorer.settle();
    lock_view(&explorer.view)
        .set_filter(&explorer.session, "helper")
        .unwrap();
    assert_eq!(explorer.command(), "Command line: not shown");
    lock_view(&explorer.view)
        .set_filter(&explorer.session, "")
        .unwrap();
    assert_eq!(explorer.command(), WORKER_A);

    explorer.select(ItemId::new(9_999));
    assert_eq!(explorer.command(), "Command line: nothing was read");
    explorer.select_with(vec![]);
    assert_eq!(explorer.command(), "Command line: nothing was read");

    // Re-selecting and refreshing re-reads the target and publishes nothing
    // new while its arguments are unchanged.
    explorer.select(explorer.item_of(4101));
    explorer.settle();
    let revision = explorer.session.current_revision();
    {
        let mut source = FakeProcessSource;
        let mut view = lock_view(&explorer.view);
        view.refresh(&explorer.session, &mut source).unwrap();
    }
    explorer.settle();
    assert!(explorer.commands_since(revision).is_empty());
    assert_eq!(explorer.command(), WORKER_A);
    let _ = &explorer.source;
}

/// The name of the body test below, which [`no_output_carries_a_secret_argument`]
/// runs in a child process.
const SECRET_BODY: &str = "secret_argument_body";
/// Set to this process's PID in the body's environment, so the body is a no-op
/// unless this test started it.
const SECRET_BODY_ENV: &str = "SRTOP_PX014_SECRET_BODY";
const SECRET_BODY_DONE: &str = "PX-014 secret-path body ran";

/// The real paths over a secret-bearing argument: the fake source wired as
/// `srtop --fake-source` wires it, every handler, the real detail worker, a
/// selection of the secret's process, a refresh that re-reads it, a sort, a
/// filter, other selections and a refusal. Ignored, and a no-op unless its
/// parent test started it. Its assertions never print what they compare.
#[test]
#[ignore = "the body of no_output_carries_a_secret_argument, run in a child process"]
fn secret_argument_body() {
    let parent = std::env::var(SECRET_BODY_ENV).unwrap_or_default();
    if parent.parse::<u32>().ok() != Some(std::os::unix::process::parent_id()) {
        return;
    }
    let explorer = Explorer::start();
    explorer.select(explorer.item_of(4101));
    explorer.settle();
    assert!(
        explorer.command().contains(FIXTURE_SECRET_ARGUMENT),
        "the secret's own process shows its argument"
    );
    {
        let mut source = FakeProcessSource;
        let mut view = lock_view(&explorer.view);
        view.refresh(&explorer.session, &mut source).unwrap();
        view.click_sort(&explorer.session, ProcessField::Name)
            .unwrap();
    }
    explorer.settle();
    explorer.select(explorer.item_of(4102));
    explorer.select(explorer.helper());
    explorer.select(ItemId::new(77));
    explorer.select(explorer.item_of(4101));
    explorer.settle();
    // A transaction the store refuses reports its error on standard error;
    // its text is the store's, never the value it carried.
    let refused = explorer.session.transaction(|ui| {
        ui.set(NodeId::new(9_999), TEXT, FIXTURE_SECRET_ARGUMENT)?;
        Ok(())
    });
    if let Err(error) = refused {
        eprintln!("body: refused transaction: {error}");
    }
    let detail = format!("{:?}", explorer.source.details().map(|_| ()));
    println!(
        "{SECRET_BODY_DONE}: selections=6 view_bytes={} detail={detail}",
        explorer.command().len()
    );
}

/// Logs do not contain secret-bearing arguments: the body above runs the real
/// paths in a child process with both of its output streams captured, and
/// neither holds any part of the secret.
#[test]
fn no_output_carries_a_secret_argument() {
    let program = std::env::current_exe().unwrap();
    let output = Command::new(&program)
        .args([
            "--exact",
            SECRET_BODY,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SECRET_BODY_ENV, std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the body runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains(SECRET) && !stderr.contains(SECRET),
        "the secret reached the body's output"
    );
    assert!(
        output.status.success(),
        "the body failed: {stdout}\n{stderr}"
    );
    assert!(stdout.contains(SECRET_BODY_DONE), "the body ran: {stdout}");
    assert!(stdout.contains("1 passed"), "the body ran: {stdout}");
}

/// No production log line prints anything but this app's own words and an
/// error: every `eprintln!` in `src/` formats exactly `{error}` (a session or
/// I/O error, whose text carries sizes and kinds, never a value), and nothing
/// outside a test prints to standard output or uses `dbg!`. A command line
/// therefore has no route to a log.
#[test]
fn no_production_log_line_formats_a_value() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // Everything before the file's test module is production code.
        let production = text.split("#[cfg(test)]\nmod tests").next().unwrap();
        for (number, line) in production.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            let without_logs = line.replace("eprintln!", "");
            for banned in [
                "println!",
                "print!(",
                "dbg!",
                "eprint!(",
                "write!(std",
                "writeln!(std",
            ] {
                assert!(
                    !without_logs.contains(banned),
                    "{}:{}: `{banned}` in production code: {line}",
                    path.display(),
                    number + 1
                );
            }
            if line.contains("eprintln!") {
                checked += 1;
                let format = line
                    .split('"')
                    .nth(1)
                    .unwrap_or_else(|| panic!("{}:{}: {line}", path.display(), number + 1));
                assert!(
                    format.starts_with("srtop: ") && format.ends_with(": {error}"),
                    "{}:{}: a log line formats only an error: {line}",
                    path.display(),
                    number + 1
                );
                assert_eq!(format.matches('{').count(), 1, "{line}");
            }
        }
    }
    assert!(checked >= 10, "the audit read the log lines ({checked})");
}
