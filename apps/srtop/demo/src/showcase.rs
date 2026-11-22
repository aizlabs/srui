//! PX-027: the R1 showcase measurements against the Linux devbox (D1 §§31–32).
//!
//! Every run reaches the box through its own [`TcpCountRelay`] in front of the
//! box's SSH port, so the encrypted stream is counted, while the probe counts
//! the semantic payload ([`crate::traffic`]). The matrices here are:
//!
//! * [`compare`]: srtop's SRUI session and htop over `ssh -tt`, alternately,
//!   on the same box, under the same process workload, at the same sampling
//!   interval (`--refresh-interval-ms` against htop's `-d`), for the same
//!   warm-up and steady window, with a fixed terminal size. The match is
//!   checked from what each side reports, not from what was asked for:
//!   srtop's interval from its own published sampling status, htop's from its
//!   argument vector in the box ([`check_matched`]).
//! * [`rtt`]: the SRUI session alone at 0/100/300/600 ms RTT, added by the
//!   relay.
//! * [`idle_check`]: an unchanged fake source: no app transaction after the
//!   snapshot for the whole window, then a canary (srtop's own `--smoke-fixture`
//!   SIGUSR1 title change) proves the session was still delivering.
//! * [`scale`]: the SRUI session alone against srtop's synthetic source.
//!
//! Workers in the box are PX-025's bounded `srtop-demo workers` (and `sleep`s
//! with a fixed lifetime); every run waits for them to be gone before the
//! next, and records how long that took.

use crate::{
    relay::Traffic,
    traffic::{
        check_payload_counts, Distribution, Split, SruiPlan, SruiProbe, SruiRun, TcpCountRelay,
        Transport, TtyProbe, TtyRun,
    },
};
use std::{
    fmt::Write as _,
    fs::File,
    net::SocketAddr,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;

/// SSH's cipher for every run, pinned so the overhead is one cipher's:
/// chacha20-poly1305 (a 4-byte encrypted length, padding, a 16-byte tag).
pub const CIPHER: &str = "chacha20-poly1305@openssh.com";
/// The box's own name for its host key, whatever port the relay listens on.
const HOST_KEY_ALIAS: &str = "[127.0.0.1]:2222";
/// The command that ends htop.
const HTOP_QUIT: &[u8] = b"q";
/// The most a run waits for its workload to end.
const WORKLOAD_END_DEADLINE: Duration = Duration::from_secs(40);
/// The settle time between starting a workload and the run.
const WORKLOAD_SETTLE: Duration = Duration::from_millis(1500);
/// Sleepers in the busy and churn workloads.
pub const SLEEPERS: u32 = 200;

/// The devbox as `devbox.sh` runs it.
#[derive(Debug, Clone)]
pub struct Devbox {
    pub docker: String,
    pub container: String,
    pub port: u16,
    pub key: PathBuf,
    pub known_hosts: PathBuf,
    /// Where each run's `ssh` standard error goes (one file per run).
    pub logs: PathBuf,
}

impl Devbox {
    fn upstream(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    /// `docker exec CONTAINER ARGS...`, its standard output.
    pub fn exec(&self, args: &[&str]) -> Result<String> {
        let output = Command::new(&self.docker)
            .arg("exec")
            .arg(&self.container)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("docker exec: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "docker exec {args:?}: {} {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Starts ARGS detached in the box as the unprivileged `srui` user.
    fn exec_detached(&self, args: &[&str]) -> Result<()> {
        let status = Command::new(&self.docker)
            .args(["exec", "-d", "-u", "srui", &self.container])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("docker exec -d: {error}"))?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| format!("docker exec -d {args:?}: {status}"))
    }

    /// Every process in the box's PID namespace, as `comm args`.
    pub fn processes(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .exec(&["ps", "-eo", "comm=,args="])?
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let (comm, args) = line.split_once(char::is_whitespace)?;
                Some((comm.to_string(), args.trim().to_string()))
            })
            .collect())
    }

    /// The one process named exactly `comm`, confirmed with `ps -p PID -o comm=`.
    pub fn confirmed_pid(&self, comm: &str) -> Result<u32> {
        let pids: Vec<u32> = self
            .exec(&["ps", "-eo", "pid=,comm="])?
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse().ok()?;
                (fields.next()? == comm).then_some(pid)
            })
            .collect();
        let [pid] = pids[..] else {
            return Err(format!("expected one {comm} in the box, found {pids:?}"));
        };
        let actual = self.exec(&["ps", "-p", &pid.to_string(), "-o", "comm="])?;
        if actual.trim() != comm {
            return Err(format!("PID {pid} is {:?}, not {comm}", actual.trim()));
        }
        Ok(pid)
    }

    /// Sends `signal` to PID only after `ps -p PID -o comm=` confirms it is `comm`.
    pub fn signal(&self, pid: u32, comm: &str, signal: &str) -> Result<()> {
        let actual = self.exec(&["ps", "-p", &pid.to_string(), "-o", "comm="])?;
        if actual.trim() != comm {
            return Err(format!(
                "PID {pid} is {:?}, not {comm}; nothing was sent",
                actual.trim()
            ));
        }
        self.exec(&["kill", &format!("-{signal}"), &pid.to_string()])
            .map(|_| ())
    }

    /// Every package version the comparison depends on, `name=version`.
    pub fn packages(&self) -> Result<String> {
        Ok(self
            .exec(&[
                "dpkg-query",
                "-W",
                "-f",
                "${Package}=${Version} ",
                "openssh-server",
                "procps",
                "htop",
            ])?
            .trim()
            .to_string())
    }

    fn ssh(&self, relay: SocketAddr, tty: bool) -> Vec<String> {
        let mut argv: Vec<String> = ["ssh", "-F", "/dev/null"].map(String::from).to_vec();
        argv.push(if tty { "-tt" } else { "-T" }.into());
        argv.extend(["-p".into(), relay.port().to_string()]);
        argv.extend(["-i".into(), self.key.display().to_string()]);
        for option in [
            "IdentitiesOnly=yes".to_string(),
            format!("UserKnownHostsFile={}", self.known_hosts.display()),
            "GlobalKnownHostsFile=/dev/null".into(),
            "StrictHostKeyChecking=yes".into(),
            "UpdateHostKeys=no".into(),
            "BatchMode=yes".into(),
            format!("HostKeyAlias={HOST_KEY_ALIAS}"),
            format!("Ciphers={CIPHER}"),
            "Compression=no".into(),
            "LogLevel=ERROR".into(),
        ] {
            argv.extend(["-o".into(), option]);
        }
        argv
    }

    /// `ssh ... -s srui@127.0.0.1 srui`: the subsystem the client uses.
    pub fn srui_argv(&self, relay: SocketAddr) -> Vec<String> {
        let mut argv = self.ssh(relay, false);
        argv.extend(["-s".into(), "srui@127.0.0.1".into(), "srui".into()]);
        argv
    }

    /// htop on a fixed-size pseudo-terminal at `delay_ds` tenths of a second.
    pub fn htop_argv(
        &self,
        relay: SocketAddr,
        delay_ds: u64,
        columns: u16,
        lines: u16,
    ) -> Vec<String> {
        let mut argv = self.ssh(relay, true);
        argv.push("srui@127.0.0.1".into());
        // Constants only: numbers this program formats, never input.
        argv.push(format!(
            "stty cols {columns} rows {lines} && exec env TERM=xterm-256color COLUMNS={columns} LINES={lines} htop -d {delay_ds}"
        ));
        argv
    }

    fn stderr(&self, name: &str) -> Stdio {
        File::create(self.logs.join(name)).map_or_else(|_| Stdio::null(), Stdio::from)
    }
}

/// A process workload, the same for both sides of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    /// The box as it is: sshd, srtop, the observer.
    Idle,
    /// [`SLEEPERS`] sleepers, a 1-thread CPU worker, a 64 MiB memory worker and
    /// a 16-thread worker: a few hundred rows, a few of them changing each tick.
    Busy,
    /// Busy, plus a short-lived-process worker: births and exits every tick.
    Churn,
}

impl Workload {
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Churn => "churn",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "idle" => Ok(Self::Idle),
            "busy" => Ok(Self::Busy),
            "churn" => Ok(Self::Churn),
            other => Err(format!("unknown workload {other:?}")),
        }
    }

    /// Starts the workload for `lifetime`; everything it starts ends by itself.
    pub fn start(self, devbox: &Devbox, lifetime: Duration) -> Result<()> {
        if self == Self::Idle {
            return Ok(());
        }
        let seconds = lifetime.as_secs().max(1);
        devbox.exec_detached(&[
            "sh",
            "-c",
            &format!("for i in $(seq 1 {SLEEPERS}); do sleep {seconds} & done; wait"),
        ])?;
        let millis = lifetime.as_millis().min(60_000).to_string();
        let mut workers = vec![
            "/usr/local/bin/srtop-demo",
            "workers",
            "--duration-ms",
            &millis,
            "--cpu-threads",
            "1",
            "--memory-mib",
            "64",
            "--threads",
            "16",
        ];
        // 64 children over the lifetime, each living 500 ms: a few births and
        // exits in every one-second sample.
        let interval = (lifetime.as_millis() / 64).clamp(10, 1000).to_string();
        if self == Self::Churn {
            workers.extend([
                "--short-lived",
                "64",
                "--short-lived-interval-ms",
                &interval,
                "--short-lived-lifetime-ms",
                "500",
            ]);
        }
        devbox.exec_detached(&workers)
    }

    /// Waits until nothing the workload started is left, and says how long that took.
    pub fn wait_ended(self, devbox: &Devbox) -> Result<Duration> {
        let start = Instant::now();
        loop {
            let left = devbox
                .processes()?
                .into_iter()
                .filter(|(comm, args)| {
                    comm.starts_with("srtop-demo")
                        || (comm == "sleep" && args.starts_with("sleep "))
                })
                .count();
            if left == 0 {
                return Ok(start.elapsed());
            }
            if start.elapsed() > WORKLOAD_END_DEADLINE {
                return Err(format!(
                    "{left} workload processes still running after {WORKLOAD_END_DEADLINE:?}"
                ));
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
}

/// The matched settings of a comparison.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub interval_ms: u64,
    pub warmup: Duration,
    pub duration: Duration,
    pub columns: u16,
    pub lines: u16,
    pub runs: usize,
}

impl Settings {
    /// htop's `-d`, in tenths of a second, for the same interval.
    pub fn htop_delay_ds(&self) -> Result<u64> {
        if !self.interval_ms.is_multiple_of(100) || self.interval_ms == 0 {
            return Err(format!(
                "htop's -d is in tenths of a second; {} ms has no exact match",
                self.interval_ms
            ));
        }
        Ok(self.interval_ms / 100)
    }

    fn lifetime(&self) -> Duration {
        WORKLOAD_SETTLE + self.warmup + self.duration + Duration::from_secs(8)
    }
}

/// One SRUI run, as the report and the checks need it.
#[derive(Debug, Clone)]
pub struct SruiRecord {
    pub workload: String,
    pub run: usize,
    pub rtt_ms: u64,
    pub processes: u64,
    pub rows: u64,
    pub interval_ms: Option<u64>,
    pub attach_ms: f64,
    pub startup_payload: u64,
    pub startup_encrypted: u64,
    pub steady_secs: f64,
    pub down: Split,
    pub up: Split,
    pub ui_transactions: u64,
    pub workload_end_ms: f64,
}

/// One htop run.
#[derive(Debug, Clone)]
pub struct HtopRecord {
    pub workload: String,
    pub run: usize,
    pub processes: u64,
    /// htop's `-d` as its argument vector in the box says, not as asked.
    pub delay_ds: Option<u64>,
    pub columns: u16,
    pub lines: u16,
    pub attach_ms: f64,
    pub startup_payload: u64,
    pub startup_encrypted: u64,
    pub steady_secs: f64,
    pub down: Split,
    pub up: Split,
    pub workload_end_ms: f64,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn total(traffic: Option<Traffic>) -> u64 {
    traffic.map_or(0, |traffic| traffic.up + traffic.down)
}

/// Turns a probe's run into a record, refusing any count that does not add up.
pub fn srui_record(
    workload: &str,
    run_no: usize,
    rtt_ms: u64,
    processes: u64,
    run: &SruiRun,
) -> Result<SruiRecord> {
    check_payload_counts(run)?;
    let steady = run
        .encrypted_steady
        .ok_or("no encrypted count for the steady window")?;
    let startup = run
        .encrypted_startup
        .ok_or("no encrypted count before the steady window")?;
    let startup_payload = run.startup.payload_down + run.startup.payload_up;
    // Startup payload and encrypted bytes are kept apart like the window's.
    Split::new(startup_payload, startup.up + startup.down)?;
    Ok(SruiRecord {
        workload: workload.to_string(),
        run: run_no,
        rtt_ms,
        processes,
        rows: run.rows,
        interval_ms: run.published_interval_ms,
        attach_ms: ms(run.attach),
        startup_payload,
        startup_encrypted: startup.up + startup.down,
        steady_secs: run.steady_duration.as_secs_f64(),
        down: Split::new(run.steady.payload_down, steady.down)?,
        up: Split::new(run.steady.payload_up, steady.up)?,
        ui_transactions: run.steady.ui_transactions,
        workload_end_ms: 0.0,
    })
}

/// Turns a terminal probe's run into a record.
#[allow(clippy::too_many_arguments)]
pub fn htop_record(
    workload: &str,
    run_no: usize,
    processes: u64,
    delay_ds: Option<u64>,
    columns: u16,
    lines: u16,
    run: &TtyRun,
) -> Result<HtopRecord> {
    let steady = run
        .encrypted_steady
        .ok_or("no encrypted count for the steady window")?;
    let startup = total(run.encrypted_startup);
    Split::new(run.startup_down, startup)?;
    Ok(HtopRecord {
        workload: workload.to_string(),
        run: run_no,
        processes,
        delay_ds,
        columns,
        lines,
        attach_ms: ms(run.attach),
        startup_payload: run.startup_down,
        startup_encrypted: startup,
        steady_secs: run.steady_duration.as_secs_f64(),
        down: Split::new(run.steady_down, steady.down)?,
        up: Split::new(run.steady_up, steady.up)?,
        workload_end_ms: 0.0,
    })
}

/// Refuses a comparison that was not matched: the same runs per workload, the
/// interval each side itself reports equal to the configured one, the same
/// steady window (within 5%), the same terminal size on every htop run, and
/// process counts within 10% of each other.
pub fn check_matched(settings: &Settings, srui: &[SruiRecord], htop: &[HtopRecord]) -> Result<()> {
    let delay = settings.htop_delay_ds()?;
    let mut problems = Vec::new();
    let workloads: Vec<&str> = {
        let mut names: Vec<&str> = srui.iter().map(|record| record.workload.as_str()).collect();
        names.dedup();
        names
    };
    for workload in &workloads {
        let ours: Vec<&SruiRecord> = srui
            .iter()
            .filter(|record| record.workload == *workload)
            .collect();
        let theirs: Vec<&HtopRecord> = htop
            .iter()
            .filter(|record| record.workload == *workload)
            .collect();
        if ours.len() != theirs.len() || ours.is_empty() {
            problems.push(format!(
                "{workload}: {} SRUI runs against {} htop runs",
                ours.len(),
                theirs.len()
            ));
            continue;
        }
        for (srui, htop) in ours.iter().zip(&theirs) {
            if srui.interval_ms != Some(settings.interval_ms) {
                problems.push(format!(
                    "{workload} run {}: srtop published an interval of {:?} ms, not {} ms",
                    srui.run, srui.interval_ms, settings.interval_ms
                ));
            }
            if htop.delay_ds != Some(delay) {
                problems.push(format!(
                    "{workload} run {}: htop ran with -d {:?}, not -d {delay} ({} ms): mismatched intervals",
                    htop.run, htop.delay_ds, settings.interval_ms
                ));
            }
            if (htop.columns, htop.lines) != (settings.columns, settings.lines) {
                problems.push(format!(
                    "{workload} run {}: terminal {}x{}",
                    htop.run, htop.columns, htop.lines
                ));
            }
            let (a, b) = (srui.steady_secs, htop.steady_secs);
            if (a - b).abs() > 0.05 * a.max(b) {
                problems.push(format!(
                    "{workload} run {}: windows of {a:.2} s and {b:.2} s",
                    srui.run
                ));
            }
            let (p, q) = (srui.processes as f64, htop.processes as f64);
            if (p - q).abs() > 0.10 * p.max(q).max(10.0) {
                problems.push(format!(
                    "{workload} run {}: {p} and {q} processes",
                    srui.run
                ));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the comparison is not matched:\n  {}",
            problems.join("\n  ")
        ))
    }
}

/// htop's `-d` from its argument vector.
pub fn htop_delay_from_args(args: &str) -> Option<u64> {
    let mut words = args.split_whitespace();
    if !words.next()?.ends_with("htop") {
        return None;
    }
    while let Some(word) = words.next() {
        if word == "-d" {
            return words.next()?.parse().ok();
        }
        if let Some(value) = word.strip_prefix("-d") {
            return value.parse().ok();
        }
    }
    None
}

fn process_count(devbox: &Devbox) -> Result<u64> {
    Ok(devbox.processes()?.len() as u64)
}

fn srui_run(
    devbox: &Devbox,
    workload: Workload,
    run_no: usize,
    rtt: Duration,
    warmup: Duration,
    duration: Duration,
    settings_lifetime: Duration,
) -> Result<SruiRecord> {
    workload.start(devbox, settings_lifetime)?;
    thread::sleep(WORKLOAD_SETTLE);
    let processes = process_count(devbox)?;
    let relay = TcpCountRelay::start(devbox.upstream(), rtt).map_err(|error| error.to_string())?;
    let sample = || relay.traffic();
    let run = SruiProbe::run(
        SruiPlan {
            transport: Transport::Command(devbox.srui_argv(relay.local_addr())),
            warmup,
            duration,
            encrypted: Some(&sample),
            canary: None,
            stderr: devbox.stderr(&format!(
                "ssh-srui-{}-{run_no}-{}ms.log",
                workload.name(),
                rtt.as_millis()
            )),
        },
        format!("px027-{}-{run_no}", workload.name()).as_bytes(),
    )?;
    let mut record = srui_record(
        workload.name(),
        run_no,
        rtt.as_millis() as u64,
        processes,
        &run,
    )?;
    record.workload_end_ms = ms(workload.wait_ended(devbox)?);
    Ok(record)
}

fn htop_run(
    devbox: &Devbox,
    workload: Workload,
    run_no: usize,
    settings: &Settings,
) -> Result<HtopRecord> {
    workload.start(devbox, settings.lifetime())?;
    thread::sleep(WORKLOAD_SETTLE);
    let processes = process_count(devbox)?;
    let relay = TcpCountRelay::start(devbox.upstream(), Duration::ZERO)
        .map_err(|error| error.to_string())?;
    let sample = || relay.traffic();
    // htop's interval as the box sees it: its own argument vector, read while it runs.
    let observed: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
    let watcher = {
        let (devbox, observed) = (devbox.clone(), Arc::clone(&observed));
        thread::spawn(move || {
            for _ in 0..40 {
                thread::sleep(Duration::from_millis(500));
                if let Ok(processes) = devbox.processes() {
                    if let Some(delay) = processes
                        .iter()
                        .filter(|(comm, _)| comm == "htop")
                        .find_map(|(_, args)| htop_delay_from_args(args))
                    {
                        *observed.lock().unwrap_or_else(|p| p.into_inner()) = Some(delay);
                        return;
                    }
                }
            }
        })
    };
    let run = TtyProbe::run(
        &devbox.htop_argv(
            relay.local_addr(),
            settings.htop_delay_ds()?,
            settings.columns,
            settings.lines,
        ),
        settings.warmup,
        settings.duration,
        HTOP_QUIT,
        Some(&sample),
        devbox.stderr(&format!("ssh-htop-{}-{run_no}.log", workload.name())),
    )?;
    let _ = watcher.join();
    let delay = *observed.lock().unwrap_or_else(|p| p.into_inner());
    let mut record = htop_record(
        workload.name(),
        run_no,
        processes,
        delay,
        settings.columns,
        settings.lines,
        &run,
    )?;
    record.workload_end_ms = ms(workload.wait_ended(devbox)?);
    Ok(record)
}

/// The comparison: for each workload, `runs` SRUI and htop runs, alternating
/// which goes first, then [`check_matched`].
pub fn compare(
    devbox: &Devbox,
    workloads: &[Workload],
    settings: &Settings,
) -> Result<(Vec<SruiRecord>, Vec<HtopRecord>)> {
    let (mut srui, mut htop) = (Vec::new(), Vec::new());
    for workload in workloads {
        for run_no in 0..settings.runs {
            let ours = |srui: &mut Vec<SruiRecord>| -> Result<()> {
                srui.push(srui_run(
                    devbox,
                    *workload,
                    run_no,
                    Duration::ZERO,
                    settings.warmup,
                    settings.duration,
                    settings.lifetime(),
                )?);
                Ok(())
            };
            if run_no % 2 == 0 {
                ours(&mut srui)?;
                htop.push(htop_run(devbox, *workload, run_no, settings)?);
            } else {
                htop.push(htop_run(devbox, *workload, run_no, settings)?);
                ours(&mut srui)?;
            }
            eprintln!("showcase: {} run {run_no} done", workload.name());
        }
    }
    check_matched(settings, &srui, &htop)?;
    Ok((srui, htop))
}

/// The SRUI session alone at each RTT.
pub fn rtt(
    devbox: &Devbox,
    workload: Workload,
    rtts_ms: &[u64],
    settings: &Settings,
) -> Result<Vec<SruiRecord>> {
    let mut records = Vec::new();
    for rtt_ms in rtts_ms {
        for run_no in 0..settings.runs {
            let record = srui_run(
                devbox,
                workload,
                run_no,
                Duration::from_millis(*rtt_ms),
                settings.warmup,
                settings.duration,
                settings.lifetime() + Duration::from_millis(*rtt_ms * 10),
            )?;
            if record.attach_ms < *rtt_ms as f64 {
                return Err(format!(
                    "rtt {rtt_ms}: attached after {:.1} ms, sooner than one RTT",
                    record.attach_ms
                ));
            }
            records.push(record);
        }
        eprintln!("showcase: rtt {rtt_ms} done");
    }
    Ok(records)
}

/// What [`idle_check`] saw.
#[derive(Debug, Clone)]
pub struct IdleReport {
    pub window: Duration,
    pub sync_revision: u64,
    pub ui_after_sync: u64,
    pub after_sync: Vec<(&'static str, u64)>,
    pub canary_after: Duration,
    pub canary_revisions: (u64, u64),
    pub canary_operations: usize,
    pub payload_down_window: u64,
    pub encrypted_window: Traffic,
}

/// Refuses an idle run that saw an app mutation, or whose canary was not the
/// very next transaction (two scalar properties on the synchronized revision).
pub fn check_idle(report: &IdleReport) -> Result<()> {
    if report.ui_after_sync != 0 {
        return Err(format!(
            "an unchanged fake source committed {} app transactions after the snapshot: {:?}",
            report.ui_after_sync, report.after_sync
        ));
    }
    if report.canary_revisions != (report.sync_revision, report.sync_revision + 1)
        || report.canary_operations != 2
    {
        return Err(format!(
            "the canary was {:?} with {} operations, not the next transaction after {}",
            report.canary_revisions, report.canary_operations, report.sync_revision
        ));
    }
    Ok(())
}

/// The unchanged fake source (`--fake-source --smoke-fixture`): one session,
/// `window` of observation after the snapshot, then srtop's own SIGUSR1 title
/// change as the canary, sent only to the PID `ps` confirms is srtop.
pub fn idle_check(devbox: &Devbox, window: Duration) -> Result<IdleReport> {
    let pid = devbox.confirmed_pid("srtop")?;
    let relay = TcpCountRelay::start(devbox.upstream(), Duration::ZERO)
        .map_err(|error| error.to_string())?;
    let sample = || relay.traffic();
    let canary = || devbox.signal(pid, "srtop", "USR1");
    let run = SruiProbe::run(
        SruiPlan {
            transport: Transport::Command(devbox.srui_argv(relay.local_addr())),
            warmup: Duration::ZERO,
            duration: window,
            encrypted: Some(&sample),
            canary: Some(&canary),
            stderr: devbox.stderr("ssh-idle.log"),
        },
        b"px027-idle",
    )?;
    check_payload_counts(&run)?;
    let canary = run.canary.clone().ok_or("no canary")?;
    let report = IdleReport {
        window: run.steady_duration,
        sync_revision: run.sync_revision,
        ui_after_sync: run.ui_after_sync,
        after_sync: run.after_sync.clone(),
        canary_after: canary.after,
        canary_revisions: canary.revisions,
        canary_operations: canary.operations,
        payload_down_window: run.steady.payload_down,
        encrypted_window: run.encrypted_steady.unwrap_or_default(),
    };
    check_idle(&report)?;
    Ok(report)
}

/// The SRUI session alone against whatever srtop the box runs (the synthetic source).
pub fn scale(devbox: &Devbox, label: &str, settings: &Settings) -> Result<Vec<SruiRecord>> {
    let mut records = Vec::new();
    for run_no in 0..settings.runs {
        let relay = TcpCountRelay::start(devbox.upstream(), Duration::ZERO)
            .map_err(|error| error.to_string())?;
        let sample = || relay.traffic();
        let run = SruiProbe::run(
            SruiPlan {
                transport: Transport::Command(devbox.srui_argv(relay.local_addr())),
                warmup: settings.warmup,
                duration: settings.duration,
                encrypted: Some(&sample),
                canary: None,
                stderr: devbox.stderr(&format!("ssh-scale-{label}-{run_no}.log")),
            },
            format!("px027-scale-{label}-{run_no}").as_bytes(),
        )?;
        records.push(srui_record(label, run_no, 0, run.rows, &run)?);
    }
    Ok(records)
}

fn dist(values: impl Iterator<Item = f64>) -> String {
    let values: Vec<f64> = values.collect();
    Distribution::of(&values).map_or_else(|| "n/a".into(), |dist| dist.cells(0))
}

fn dist1(values: impl Iterator<Item = f64>) -> String {
    let values: Vec<f64> = values.collect();
    Distribution::of(&values).map_or_else(|| "n/a".into(), |dist| dist.cells(1))
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<f64> = values.collect();
    Distribution::of(&values).map_or(f64::NAN, |dist| dist.median)
}

fn rate(split: Split, secs: f64, encrypted: bool) -> f64 {
    (if encrypted {
        split.encrypted
    } else {
        split.payload
    }) as f64
        / secs
}

/// The comparison as Markdown: every figure is min / median / p95 / max over the runs.
pub fn compare_markdown(settings: &Settings, srui: &[SruiRecord], htop: &[HtopRecord]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Steady window {:.0} s after a {:.0} s warm-up; sampling {} ms (srtop `--refresh-interval-ms {}`, htop `-d {}`); htop on a {}x{} terminal; {} runs per side per workload. Bytes per second; min / median / p95 / max.\n",
        settings.duration.as_secs_f64(),
        settings.warmup.as_secs_f64(),
        settings.interval_ms,
        settings.interval_ms,
        settings.interval_ms / 100,
        settings.columns,
        settings.lines,
        settings.runs
    );
    let _ = writeln!(out, "| Workload | Side | Processes | Payload down B/s | Encrypted down B/s | SSH overhead down B/s | Payload up B/s | Encrypted up B/s | Startup payload B | Startup encrypted B | Attach ms |");
    let _ = writeln!(
        out,
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    let mut workloads: Vec<&str> = srui.iter().map(|record| record.workload.as_str()).collect();
    workloads.dedup();
    for workload in &workloads {
        let ours: Vec<&SruiRecord> = srui
            .iter()
            .filter(|record| record.workload == *workload)
            .collect();
        let theirs: Vec<&HtopRecord> = htop
            .iter()
            .filter(|record| record.workload == *workload)
            .collect();
        let _ = writeln!(
            out,
            "| {workload} | SRUI (srtop) | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            dist(ours.iter().map(|r| r.processes as f64)),
            dist(ours.iter().map(|r| rate(r.down, r.steady_secs, false))),
            dist(ours.iter().map(|r| rate(r.down, r.steady_secs, true))),
            dist(
                ours.iter()
                    .map(|r| r.down.overhead() as f64 / r.steady_secs)
            ),
            dist(ours.iter().map(|r| rate(r.up, r.steady_secs, false))),
            dist(ours.iter().map(|r| rate(r.up, r.steady_secs, true))),
            dist(ours.iter().map(|r| r.startup_payload as f64)),
            dist(ours.iter().map(|r| r.startup_encrypted as f64)),
            dist1(ours.iter().map(|r| r.attach_ms)),
        );
        let _ = writeln!(
            out,
            "| {workload} | htop over ssh -tt | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            dist(theirs.iter().map(|r| r.processes as f64)),
            dist(theirs.iter().map(|r| rate(r.down, r.steady_secs, false))),
            dist(theirs.iter().map(|r| rate(r.down, r.steady_secs, true))),
            dist(
                theirs
                    .iter()
                    .map(|r| r.down.overhead() as f64 / r.steady_secs)
            ),
            dist(theirs.iter().map(|r| rate(r.up, r.steady_secs, false))),
            dist(theirs.iter().map(|r| rate(r.up, r.steady_secs, true))),
            dist(theirs.iter().map(|r| r.startup_payload as f64)),
            dist(theirs.iter().map(|r| r.startup_encrypted as f64)),
            dist1(theirs.iter().map(|r| r.attach_ms)),
        );
    }
    let _ = writeln!(
        out,
        "\nMedian encrypted bytes per second, both directions, and which side is larger:\n"
    );
    let _ = writeln!(out, "| Workload | SRUI rows | SRUI encrypted B/s | htop encrypted B/s | Larger | Ratio SRUI/htop | Workers gone after (max ms) |");
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- | --- |");
    for workload in &workloads {
        let ours: Vec<&SruiRecord> = srui
            .iter()
            .filter(|record| record.workload == *workload)
            .collect();
        let theirs: Vec<&HtopRecord> = htop
            .iter()
            .filter(|record| record.workload == *workload)
            .collect();
        let a = median(
            ours.iter()
                .map(|r| (r.down.encrypted + r.up.encrypted) as f64 / r.steady_secs),
        );
        let b = median(
            theirs
                .iter()
                .map(|r| (r.down.encrypted + r.up.encrypted) as f64 / r.steady_secs),
        );
        let gone = ours
            .iter()
            .map(|r| r.workload_end_ms)
            .chain(theirs.iter().map(|r| r.workload_end_ms))
            .fold(0.0_f64, f64::max);
        let _ = writeln!(
            out,
            "| {workload} | {} | {a:.0} | {b:.0} | {} | {:.2} | {gone:.0} |",
            dist(ours.iter().map(|r| r.rows as f64)),
            if a > b { "SRUI" } else { "htop" },
            a / b
        );
    }
    out
}

/// The RTT matrix as Markdown.
pub fn rtt_markdown(settings: &Settings, records: &[SruiRecord]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "SRUI session over real SSH through the TCP relay, RTT added by the relay (RTT/2 each way); {} runs per RTT; steady window {:.0} s after {:.0} s. min / median / p95 / max.\n",
        settings.runs,
        settings.duration.as_secs_f64(),
        settings.warmup.as_secs_f64()
    );
    let _ = writeln!(out, "| RTT ms | Rows | Attach ms (ssh start to snapshot applied) | Startup payload B | Startup encrypted B | Payload down B/s | Encrypted down B/s | App transactions in window |");
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut rtts: Vec<u64> = records.iter().map(|record| record.rtt_ms).collect();
    rtts.dedup();
    for rtt in rtts {
        let rows: Vec<&SruiRecord> = records
            .iter()
            .filter(|record| record.rtt_ms == rtt)
            .collect();
        let _ = writeln!(
            out,
            "| {rtt} | {} | {} | {} | {} | {} | {} | {} |",
            dist(rows.iter().map(|r| r.rows as f64)),
            dist1(rows.iter().map(|r| r.attach_ms)),
            dist(rows.iter().map(|r| r.startup_payload as f64)),
            dist(rows.iter().map(|r| r.startup_encrypted as f64)),
            dist(rows.iter().map(|r| rate(r.down, r.steady_secs, false))),
            dist(rows.iter().map(|r| rate(r.down, r.steady_secs, true))),
            dist(rows.iter().map(|r| r.ui_transactions as f64)),
        );
    }
    out
}

/// The scale runs as one Markdown row each label.
pub fn scale_markdown(label: &str, records: &[SruiRecord]) -> String {
    format!(
        "| {label} | {} | {} | {} | {} | {} | {} | {} |\n",
        dist(records.iter().map(|r| r.rows as f64)),
        dist1(records.iter().map(|r| r.attach_ms)),
        dist(records.iter().map(|r| r.startup_payload as f64)),
        dist(records.iter().map(|r| r.startup_encrypted as f64)),
        dist(records.iter().map(|r| rate(r.down, r.steady_secs, false))),
        dist(records.iter().map(|r| rate(r.down, r.steady_secs, true))),
        dist(
            records
                .iter()
                .map(|r| r.down.overhead() as f64 / r.steady_secs)
        ),
    )
}

fn json_split(split: Split) -> String {
    format!(
        "{{\"payload\": {}, \"encrypted\": {}, \"overhead\": {}}}",
        split.payload,
        split.encrypted,
        split.overhead()
    )
}

/// One JSON object per SRUI record.
pub fn srui_json(record: &SruiRecord) -> String {
    format!(
        "{{\"side\": \"srui\", \"workload\": \"{}\", \"run\": {}, \"rtt_ms\": {}, \"processes\": {}, \"rows\": {}, \"published_interval_ms\": {}, \"attach_ms\": {:.1}, \"startup_payload\": {}, \"startup_encrypted\": {}, \"steady_secs\": {:.3}, \"down\": {}, \"up\": {}, \"app_transactions_in_window\": {}, \"workload_end_ms\": {:.0}}}",
        record.workload,
        record.run,
        record.rtt_ms,
        record.processes,
        record.rows,
        record.interval_ms.map_or("null".into(), |ms| ms.to_string()),
        record.attach_ms,
        record.startup_payload,
        record.startup_encrypted,
        record.steady_secs,
        json_split(record.down),
        json_split(record.up),
        record.ui_transactions,
        record.workload_end_ms,
    )
}

/// One JSON object per htop record.
pub fn htop_json(record: &HtopRecord) -> String {
    format!(
        "{{\"side\": \"htop\", \"workload\": \"{}\", \"run\": {}, \"processes\": {}, \"observed_delay_ds\": {}, \"terminal\": \"{}x{}\", \"attach_ms\": {:.1}, \"startup_payload\": {}, \"startup_encrypted\": {}, \"steady_secs\": {:.3}, \"down\": {}, \"up\": {}, \"workload_end_ms\": {:.0}}}",
        record.workload,
        record.run,
        record.processes,
        record.delay_ds.map_or("null".into(), |d| d.to_string()),
        record.columns,
        record.lines,
        record.attach_ms,
        record.startup_payload,
        record.startup_encrypted,
        record.steady_secs,
        json_split(record.down),
        json_split(record.up),
        record.workload_end_ms,
    )
}

/// The idle check as one JSON object.
pub fn idle_json(report: &IdleReport) -> String {
    let after: Vec<String> = report
        .after_sync
        .iter()
        .map(|(kind, bytes)| format!("\"{kind} ({bytes} B)\""))
        .collect();
    format!(
        "{{\"window_secs\": {:.3}, \"sync_revision\": {}, \"app_transactions_after_sync\": {}, \"messages_after_sync\": [{}], \"payload_down_window\": {}, \"encrypted_window\": {{\"up\": {}, \"down\": {}}}, \"canary\": {{\"after_ms\": {:.1}, \"revisions\": [{}, {}], \"operations\": {}}}}}",
        report.window.as_secs_f64(),
        report.sync_revision,
        report.ui_after_sync,
        after.join(", "),
        report.payload_down_window,
        report.encrypted_window.up,
        report.encrypted_window.down,
        ms(report.canary_after),
        report.canary_revisions.0,
        report.canary_revisions.1,
        report.canary_operations,
    )
}
