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
//!     [--threads N] [--short-lived COUNT]
//!     Starts one worker per requested mode under one fresh owner token,
//!     prints `owner TOKEN` and `worker MODE PID`, and waits for them all to
//!     end. However this process ends — normally, SIGINT, SIGTERM or SIGKILL —
//!     its workers end with it (their lifelines close).
//!
//! srtop-demo scenarios --srtop PATH [--trials N]
//!     Runs the remote-result matrix at 0/100/300/600 ms RTT, a transport
//!     interruption and a collector failure against srtop's own fixtures,
//!     and prints one JSON report.
//! ```

use srtop_demo::{
    harness::{fresh_owner, DemoWorker},
    relay::DelayRelay,
    scenario::{self, RTTS_MS},
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
    for (flag, value) in flag_values(args)? {
        let value = number(&flag, &value)?;
        let small = u32::try_from(value).map_err(|_| format!("{flag} is out of range"))?;
        match flag.as_str() {
            "--duration-ms" => duration = Some(Duration::from_millis(value)),
            "--cpu-threads" => modes.push(Mode::Cpu { threads: small }),
            "--memory-mib" => modes.push(Mode::Memory { mib: small }),
            "--threads" => modes.push(Mode::Threads { count: small }),
            "--short-lived" => modes.push(Mode::ShortLived {
                count: small,
                lifetime: Duration::from_millis(500),
                interval: Duration::from_millis(100),
            }),
            other => return Err(format!("unknown argument {other:?}")),
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("relay") => relay(&args[1..]),
        Some("workers") => workers(&args[1..]),
        Some("scenarios") => scenarios(&args[1..]),
        _ => Err("usage: srtop-demo relay|workers|scenarios … (see the source header)".into()),
    };
    if let Err(error) = result {
        eprintln!("srtop-demo: {error}");
        std::process::exit(2);
    }
}
