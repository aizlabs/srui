//! `srtop-demo`: the owner side of PX-025's demonstrations.
//!
//! ```text
//! srtop-demo relay --listen PATH --upstream PATH --rtt-ms MS
//!     A delay relay (RTT/2 each way). Prints `ready` once listening. Reads
//!     commands on standard input: `cut` cuts every live connection and prints
//!     `cut N`; `traffic` prints `traffic up=B down=B accepted=N`. End of file
//!     on standard input stops it and removes its socket.
//!
//! srtop-demo workers --duration-ms MS [--cpu-threads N] [--memory-mib N]
//!     [--threads N] [--short-lived COUNT [--short-lived-interval-ms MS]
//!     [--short-lived-lifetime-ms MS]]
//!     Starts one worker per requested mode under one fresh owner token (each
//!     flag at most once, so at most four workers), prints `owner TOKEN` and
//!     `worker MODE PID`, and waits for them all to
//!     end. However this process ends — normally, SIGINT, SIGTERM or SIGKILL —
//!     its workers end with it (their lifelines close).
//!
//! srtop-demo scenarios --srtop PATH [--trials N]
//!     Runs the remote-result matrix at 0/100/300/600 ms RTT, a transport
//!     interruption and a collector failure against srtop's own fixtures,
//!     and prints one JSON report.
//!
//! srtop-demo showcase compare|rtt|idle|scale --key PATH --known-hosts PATH
//!     --logs DIR --out FILE [--docker PATH] [--container NAME] [--port N]
//!     [--runs N] [--warmup-ms MS] [--duration-ms MS] [--interval-ms MS]
//!     [--columns N] [--lines N] [--workloads idle,busy,churn]
//!     [--rtts 0,100,300,600] [--label TEXT]
//!     PX-027's measurements against the devbox (`showcase` module): writes
//!     Markdown to FILE and one JSON object per run to FILE.json, and exits
//!     non-zero if a run's counts do not add up or the comparison is not
//!     matched.
//! ```

use srtop_demo::{
    harness::{fresh_owner, DemoWorker},
    relay::DelayRelay,
    scenario::{self, RTTS_MS},
    showcase::{self, Devbox, Settings, Workload},
    worker::{Mode, WorkerSpec},
};
use std::{
    io::{BufRead, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, String>;

fn flag_values(args: &[String]) -> Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(|| format!("{flag} needs a value"))?;
        pairs.push((flag.clone(), value.clone()));
    }
    Ok(pairs)
}

fn number(flag: &str, value: &str) -> Result<u64> {
    value
        .parse()
        .map_err(|_| format!("{flag} takes a number, got {value:?}"))
}

fn relay(args: &[String]) -> Result<()> {
    let (mut listen, mut upstream, mut rtt) = (None, None, None);
    for (flag, value) in flag_values(args)? {
        match flag.as_str() {
            "--listen" => listen = Some(PathBuf::from(value)),
            "--upstream" => upstream = Some(PathBuf::from(value)),
            "--rtt-ms" => rtt = Some(Duration::from_millis(number(&flag, &value)?)),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let (listen, upstream, rtt) = (
        listen.ok_or("relay needs --listen PATH")?,
        upstream.ok_or("relay needs --upstream PATH")?,
        rtt.ok_or("relay needs --rtt-ms MS")?,
    );
    let mut relay =
        DelayRelay::start(&listen, &upstream, rtt).map_err(|error| error.to_string())?;
    let (up, down) = relay.one_way();
    let mut out = std::io::stdout();
    let say = |out: &mut std::io::Stdout, line: String| {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    say(
        &mut out,
        format!(
            "ready rtt_ms={} up_ms={} down_ms={}",
            rtt.as_millis(),
            up.as_millis(),
            down.as_millis()
        ),
    );
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        match line.trim() {
            "cut" => {
                let cut = relay.cut();
                say(&mut out, format!("cut {cut}"));
            }
            "traffic" => {
                let traffic = relay.traffic();
                say(
                    &mut out,
                    format!(
                        "traffic up={} down={} accepted={}",
                        traffic.up,
                        traffic.down,
                        relay.accepted()
                    ),
                );
            }
            "" => {}
            other => say(&mut out, format!("unknown command {other:?}")),
        }
    }
    relay.stop();
    Ok(())
}

fn workers(args: &[String]) -> Result<()> {
    let owner = fresh_owner();
    let mut duration = None;
    let mut modes = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    // PX-027: the short-lived worker's pace, so a run can spread its 64
    // children over its own duration. Validated with the rest of the spec.
    let (mut lifetime, mut interval) = (Duration::from_millis(500), Duration::from_millis(100));
    for (flag, value) in flag_values(args)? {
        // One worker per named workload: a repeated flag would multiply the
        // per-worker limits, so a run is at most one worker of each mode.
        if seen.contains(&flag) {
            return Err(format!("{flag} given twice: one worker per workload"));
        }
        seen.push(flag.clone());
        let value = number(&flag, &value)?;
        let small = u32::try_from(value).map_err(|_| format!("{flag} is out of range"))?;
        match flag.as_str() {
            "--duration-ms" => duration = Some(Duration::from_millis(value)),
            "--cpu-threads" => modes.push(Mode::Cpu { threads: small }),
            "--memory-mib" => modes.push(Mode::Memory { mib: small }),
            "--threads" => modes.push(Mode::Threads { count: small }),
            "--short-lived" => modes.push(Mode::ShortLived {
                count: small,
                lifetime,
                interval,
            }),
            "--short-lived-interval-ms" => interval = Duration::from_millis(value),
            "--short-lived-lifetime-ms" => lifetime = Duration::from_millis(value),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    for mode in &mut modes {
        if let Mode::ShortLived {
            lifetime: l,
            interval: i,
            ..
        } = mode
        {
            (*l, *i) = (lifetime, interval);
        }
    }
    let duration = duration.ok_or("workers needs --duration-ms MS")?;
    if modes.is_empty() {
        return Err("name at least one workload".into());
    }
    let specs: Vec<WorkerSpec> = modes
        .into_iter()
        .map(|mode| WorkerSpec {
            owner: owner.clone(),
            mode,
            duration,
            fail_after: None,
        })
        .collect();
    for spec in &specs {
        spec.validate()?;
    }
    let exe = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name("srtop-demo-worker");
    let mut out = std::io::stdout();
    let _ = writeln!(out, "owner {owner}");
    let mut running = Vec::new();
    for spec in &specs {
        let worker = DemoWorker::spawn(&exe, spec).map_err(|error| error.to_string())?;
        let _ = writeln!(out, "worker {} {}", spec.mode.name(), worker.pid());
        running.push((spec.mode.name(), worker));
    }
    let _ = out.flush();
    for (mode, mut worker) in running {
        let status = worker
            .wait_timeout(duration + Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        let _ = writeln!(out, "ended {mode} {status:?}");
    }
    Ok(())
}

/// One srtop started for a scenario, in a private directory; killed, reaped
/// and its directory removed on drop.
struct Srtop {
    child: Child,
    directory: PathBuf,
}

impl Srtop {
    fn start(binary: &Path, options: &[&str]) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let directory = PathBuf::from(format!("/tmp/srtop-demo-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&directory).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let socket = directory.join("s.sock");
        let child = Command::new(binary)
            .arg("--socket")
            .arg(&socket)
            .args(options)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("start {}: {error}", binary.display()))?;
        let mut srtop = Self { child, directory };
        let deadline = Instant::now() + Duration::from_secs(120);
        while UnixStream::connect(&socket).is_err() {
            if !matches!(srtop.child.try_wait(), Ok(None)) || Instant::now() >= deadline {
                return Err("srtop did not become ready".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(srtop)
    }

    fn socket(&self) -> PathBuf {
        self.directory.join("s.sock")
    }

    fn relay_socket(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
}

impl Drop for Srtop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn scenarios(args: &[String]) -> Result<()> {
    let (mut binary, mut trials) = (None, 5_usize);
    for (flag, value) in flag_values(args)? {
        match flag.as_str() {
            "--srtop" => binary = Some(PathBuf::from(value)),
            "--trials" => {
                trials = usize::try_from(number(&flag, &value)?).map_err(|e| e.to_string())?
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let binary = binary.ok_or("scenarios needs --srtop PATH")?;
    let names = Srtop::start(&binary, &["--fake-repeated-names"])?;
    let mut remote = Vec::new();
    for rtt in RTTS_MS {
        let report = scenario::remote_results(
            &names.socket(),
            &names.relay_socket(&format!("r{rtt}.sock")),
            Duration::from_millis(rtt),
            trials,
        )?;
        remote.push(report.to_json());
    }
    let sequence = Srtop::start(
        &binary,
        &["--fake-sequence", "--refresh-interval-ms", "100"],
    )?;
    let mut interruptions = Vec::new();
    for rtt in RTTS_MS {
        let report = scenario::transport_interruption(
            &sequence.socket(),
            &sequence.relay_socket(&format!("i{rtt}.sock")),
            Duration::from_millis(rtt),
            Duration::from_millis(300),
        )?;
        interruptions.push(report.to_json());
    }
    let failure = scenario::collector_failure(
        &sequence.socket(),
        &sequence.relay_socket("f.sock"),
        Duration::from_millis(300),
    )?;
    println!(
        "{{\"remote_results\": [{}], \"transport_interruption\": [{}], \"collector_failure\": {}}}",
        remote.join(", "),
        interruptions.join(", "),
        failure.to_json()
    );
    Ok(())
}

fn showcase(args: &[String]) -> Result<()> {
    let (matrix, rest) = args
        .split_first()
        .ok_or("showcase needs compare|rtt|idle|scale")?;
    let mut devbox = Devbox {
        docker: "docker".into(),
        container: "srtop-devbox".into(),
        port: 2222,
        key: PathBuf::new(),
        known_hosts: PathBuf::new(),
        logs: PathBuf::new(),
    };
    let mut settings = Settings {
        interval_ms: 1000,
        warmup: Duration::from_secs(3),
        duration: Duration::from_secs(15),
        columns: 120,
        lines: 40,
        runs: 7,
    };
    let (mut out, mut workloads, mut rtts, mut label) = (
        None,
        vec![Workload::Idle, Workload::Busy, Workload::Churn],
        RTTS_MS.to_vec(),
        "scale".to_string(),
    );
    for (flag, value) in flag_values(rest)? {
        match flag.as_str() {
            "--docker" => devbox.docker = value,
            "--container" => devbox.container = value,
            "--port" => {
                devbox.port = u16::try_from(number(&flag, &value)?).map_err(|e| e.to_string())?
            }
            "--key" => devbox.key = PathBuf::from(value),
            "--known-hosts" => devbox.known_hosts = PathBuf::from(value),
            "--logs" => devbox.logs = PathBuf::from(value),
            "--out" => out = Some(PathBuf::from(value)),
            "--runs" => {
                settings.runs =
                    usize::try_from(number(&flag, &value)?).map_err(|e| e.to_string())?
            }
            "--warmup-ms" => settings.warmup = Duration::from_millis(number(&flag, &value)?),
            "--duration-ms" => settings.duration = Duration::from_millis(number(&flag, &value)?),
            "--interval-ms" => settings.interval_ms = number(&flag, &value)?,
            "--columns" => {
                settings.columns =
                    u16::try_from(number(&flag, &value)?).map_err(|e| e.to_string())?
            }
            "--lines" => {
                settings.lines = u16::try_from(number(&flag, &value)?).map_err(|e| e.to_string())?
            }
            "--workloads" => {
                workloads = value
                    .split(',')
                    .map(Workload::parse)
                    .collect::<Result<_>>()?
            }
            "--rtts" => {
                rtts = value
                    .split(',')
                    .map(|rtt| number(&flag, rtt))
                    .collect::<Result<_>>()?
            }
            "--label" => {
                // Bounded: it names a row of the report and a JSON string, nothing more.
                if value.is_empty() || value.len() > 64 {
                    return Err("--label takes 1 to 64 bytes".into());
                }
                label = value
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let out = out.ok_or("showcase needs --out FILE")?;
    for (what, path) in [
        ("--key", &devbox.key),
        ("--known-hosts", &devbox.known_hosts),
    ] {
        if !path.is_file() {
            return Err(format!("{what} {} is not a file", path.display()));
        }
    }
    if !devbox.logs.is_dir() {
        return Err(format!(
            "--logs {} is not a directory",
            devbox.logs.display()
        ));
    }
    if settings.runs == 0 {
        return Err("--runs must be at least 1".into());
    }
    let (markdown, json): (String, Vec<String>) = match matrix.as_str() {
        "compare" => {
            let (srui, htop) = showcase::compare(&devbox, &workloads, &settings)?;
            (
                showcase::compare_markdown(&settings, &srui, &htop),
                srui.iter()
                    .map(showcase::srui_json)
                    .chain(htop.iter().map(showcase::htop_json))
                    .collect(),
            )
        }
        "rtt" => {
            let workload = *workloads.first().ok_or("rtt needs one workload")?;
            let records = showcase::rtt(&devbox, workload, &rtts, &settings)?;
            (
                showcase::rtt_markdown(&settings, &records),
                records.iter().map(showcase::srui_json).collect(),
            )
        }
        "idle" => {
            let report = showcase::idle_check(&devbox, settings.duration)?;
            (
                format!(
                    "Unchanged fake source: {:.1} s after the snapshot at revision {}, {} app transactions; canary {} -> {} ({} operations) {:.1} ms after SIGUSR1.\n",
                    report.window.as_secs_f64(),
                    report.sync_revision,
                    report.ui_after_sync,
                    report.canary_revisions.0,
                    report.canary_revisions.1,
                    report.canary_operations,
                    report.canary_after.as_secs_f64() * 1000.0
                ),
                vec![showcase::idle_json(&report)],
            )
        }
        "scale" => {
            let records = showcase::scale(&devbox, &label, &settings)?;
            (
                showcase::scale_markdown(&label, &records),
                records.iter().map(showcase::srui_json).collect(),
            )
        }
        other => return Err(format!("unknown showcase matrix {other:?}")),
    };
    std::fs::write(&out, &markdown).map_err(|error| format!("write {}: {error}", out.display()))?;
    let json_path = out.with_extension("json");
    std::fs::write(&json_path, format!("[\n{}\n]\n", json.join(",\n")))
        .map_err(|error| format!("write {}: {error}", json_path.display()))?;
    print!("{markdown}");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("relay") => relay(&args[1..]),
        Some("workers") => workers(&args[1..]),
        Some("scenarios") => scenarios(&args[1..]),
        Some("showcase") => showcase(&args[1..]),
        _ => Err(
            "usage: srtop-demo relay|workers|scenarios|showcase … (see the source header)".into(),
        ),
    };
    if let Err(error) = result {
        eprintln!("srtop-demo: {error}");
        std::process::exit(2);
    }
}
