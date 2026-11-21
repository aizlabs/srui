//! The opt-in demonstration worker (PX-025; D1 §31.4; D2 T34).
//!
//! A worker is a disposable process a test or a demonstration starts so that
//! srtop has something real and harmless to show. It never targets another
//! process: it only burns CPU, holds memory, parks threads or starts and reaps
//! short-lived children of its own.
//!
//! Every worker is bounded three ways, whatever its owner does:
//!
//! * **Test-owned.** It refuses to start unless it is given an owner token
//!   (`--srtop-test-owner srtop-demo-…`) and the same token in
//!   [`OWNER_ENV`]. The token stays in its argument vector, so a sweep can
//!   find exactly one run's processes by it ([`crate::harness::processes_with_owner`]).
//! * **Hard limits.** Every size is validated against a fixed ceiling
//!   ([`MAX_DURATION`], [`MAX_CPU_THREADS`], [`MAX_MEMORY_MIB`],
//!   [`MAX_THREADS`], [`MAX_CHILDREN`], [`MAX_LIVE_CHILDREN`],
//!   [`MAX_CHILD_LIFETIME`]); a value past one is refused, not clamped.
//! * **It ends by itself.** The main thread waits for the deadline and then
//!   exits the process, so a worker whose owner died still ends on time. Its
//!   standard input is a lifeline: end of file (the owner exited, however it
//!   was stopped, including SIGKILL) ends it at once. A short-lived child's
//!   lifeline is its worker, so the chain unwinds the same way.

use std::{
    io::BufRead,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// The flag that carries the owner token in a worker's argument vector.
pub const OWNER_FLAG: &str = "--srtop-test-owner";
/// The environment variable that must carry the same token.
pub const OWNER_ENV: &str = "SRTOP_DEMO_WORKER_OWNER";
/// Every owner token starts with this.
pub const OWNER_PREFIX: &str = "srtop-demo-";
/// The longest owner token accepted.
pub const MAX_OWNER_LEN: usize = 64;

/// The longest a worker may run.
pub const MAX_DURATION: Duration = Duration::from_secs(60);
/// The most spinning threads a CPU worker may start.
pub const MAX_CPU_THREADS: u32 = 4;
/// The most memory a memory worker may hold, in MiB.
pub const MAX_MEMORY_MIB: u32 = 256;
/// The most parked threads a thread worker may start.
pub const MAX_THREADS: u32 = 64;
/// The most children a short-lived-process worker may start in total.
pub const MAX_CHILDREN: u32 = 64;
/// The most of those children alive at one time.
pub const MAX_LIVE_CHILDREN: u32 = 8;
/// The longest one short-lived child may live.
pub const MAX_CHILD_LIFETIME: Duration = Duration::from_secs(2);
/// The shortest and longest pause between two children.
pub const MIN_SPAWN_INTERVAL: Duration = Duration::from_millis(10);
pub const MAX_SPAWN_INTERVAL: Duration = Duration::from_secs(1);

/// How a worker ended, as its exit status.
pub mod exit {
    /// It reached its deadline.
    pub const COMPLETED: i32 = 0;
    /// Its specification was refused; it did nothing.
    pub const REFUSED: i32 = 2;
    /// Its owner asked it to stop (`stop` on its standard input).
    pub const STOPPED: i32 = 20;
    /// Its lifeline closed: the owner is gone.
    pub const ORPHANED: i32 = 21;
    /// It was asked to fail (`--fail-after-ms`), as a failing workload does.
    pub const FAILED: i32 = 22;
}

/// What a worker does until it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Spins `threads` threads.
    Cpu { threads: u32 },
    /// Allocates `mib` MiB and touches every page, so it is resident.
    Memory { mib: u32 },
    /// Starts `count` threads that only park.
    Threads { count: u32 },
    /// Starts `count` children, one every `interval`, each living `lifetime`
    /// (at most [`MAX_LIVE_CHILDREN`] at once), and reaps them.
    ShortLived {
        count: u32,
        lifetime: Duration,
        interval: Duration,
    },
    /// One short-lived child of a [`Mode::ShortLived`] worker; it only waits.
    Child,
}

impl Mode {
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Cpu { .. } => "cpu",
            Mode::Memory { .. } => "memory",
            Mode::Threads { .. } => "threads",
            Mode::ShortLived { .. } => "short-lived",
            Mode::Child => "child",
        }
    }
}

/// A validated worker specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSpec {
    pub owner: String,
    pub mode: Mode,
    /// The hard deadline, from start.
    pub duration: Duration,
    /// Fail (exit [`exit::FAILED`]) this long after start, before the deadline.
    pub fail_after: Option<Duration>,
}

/// Whether `token` is a well-formed owner token.
pub fn valid_owner(token: &str) -> bool {
    token.len() > OWNER_PREFIX.len()
        && token.len() <= MAX_OWNER_LEN
        && token.starts_with(OWNER_PREFIX)
        && token
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn bounded(name: &str, value: u64, min: u64, max: u64) -> Result<u64, String> {
    if value < min || value > max {
        return Err(format!("{name} must be {min}..={max}, got {value}"));
    }
    Ok(value)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl WorkerSpec {
    /// Checks every bound; a value past one is refused, never clamped.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_owner(&self.owner) {
            return Err(format!(
                "the owner token must be {OWNER_PREFIX}… ([a-z0-9-], at most {MAX_OWNER_LEN} bytes)"
            ));
        }
        let max_duration = if self.mode == Mode::Child {
            MAX_CHILD_LIFETIME
        } else {
            MAX_DURATION
        };
        bounded(
            "--duration-ms",
            millis(self.duration),
            1,
            millis(max_duration),
        )?;
        if let Some(after) = self.fail_after {
            bounded("--fail-after-ms", millis(after), 1, millis(self.duration))?;
        }
        match &self.mode {
            Mode::Cpu { threads } => {
                bounded(
                    "--threads",
                    u64::from(*threads),
                    1,
                    u64::from(MAX_CPU_THREADS),
                )?;
            }
            Mode::Memory { mib } => {
                bounded("--mib", u64::from(*mib), 1, u64::from(MAX_MEMORY_MIB))?;
            }
            Mode::Threads { count } => {
                bounded("--threads", u64::from(*count), 1, u64::from(MAX_THREADS))?;
            }
            Mode::ShortLived {
                count,
                lifetime,
                interval,
            } => {
                bounded("--count", u64::from(*count), 1, u64::from(MAX_CHILDREN))?;
                bounded(
                    "--lifetime-ms",
                    millis(*lifetime),
                    1,
                    millis(MAX_CHILD_LIFETIME),
                )?;
                bounded(
                    "--interval-ms",
                    millis(*interval),
                    millis(MIN_SPAWN_INTERVAL),
                    millis(MAX_SPAWN_INTERVAL),
                )?;
            }
            Mode::Child => {}
        }
        Ok(())
    }

    /// The worker's argument vector, owner flag first.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            OWNER_FLAG.to_string(),
            self.owner.clone(),
            "--mode".into(),
            self.mode.name().into(),
            "--duration-ms".into(),
            millis(self.duration).to_string(),
        ];
        match &self.mode {
            Mode::Cpu { threads } => args.extend(["--threads".into(), threads.to_string()]),
            Mode::Memory { mib } => args.extend(["--mib".into(), mib.to_string()]),
            Mode::Threads { count } => args.extend(["--threads".into(), count.to_string()]),
            Mode::ShortLived {
                count,
                lifetime,
                interval,
            } => args.extend([
                "--count".into(),
                count.to_string(),
                "--lifetime-ms".into(),
                millis(*lifetime).to_string(),
                "--interval-ms".into(),
                millis(*interval).to_string(),
            ]),
            Mode::Child => {}
        }
        if let Some(after) = self.fail_after {
            args.extend(["--fail-after-ms".into(), millis(after).to_string()]);
        }
        args
    }

    /// Parses and validates a worker's arguments. `env_owner` is the value of
    /// [`OWNER_ENV`], which must name the same token as [`OWNER_FLAG`].
    pub fn parse(args: &[String], env_owner: Option<&str>) -> Result<Self, String> {
        let mut owner = None;
        let mut mode = None;
        let mut duration = None;
        let mut fail_after = None;
        let (mut threads, mut mib, mut count, mut lifetime, mut interval) =
            (None, None, None, None, None);
        let mut rest = args.iter();
        while let Some(flag) = rest.next() {
            let value = rest.next().ok_or_else(|| format!("{flag} needs a value"))?;
            let number = || {
                value
                    .parse::<u64>()
                    .map_err(|_| format!("{flag} takes a number, got {value:?}"))
            };
            let slot = match flag.as_str() {
                OWNER_FLAG => {
                    owner = Some(value.clone());
                    continue;
                }
                "--mode" => {
                    mode = Some(value.clone());
                    continue;
                }
                "--duration-ms" => &mut duration,
                "--fail-after-ms" => &mut fail_after,
                "--threads" => &mut threads,
                "--mib" => &mut mib,
                "--count" => &mut count,
                "--lifetime-ms" => &mut lifetime,
                "--interval-ms" => &mut interval,
                other => return Err(format!("unknown argument {other:?}")),
            };
            if slot.replace(number()?).is_some() {
                return Err(format!("{flag} given twice"));
            }
        }
        let owner = owner.ok_or_else(|| format!("a worker needs {OWNER_FLAG} TOKEN"))?;
        if env_owner != Some(owner.as_str()) {
            return Err(format!("{OWNER_ENV} must equal the {OWNER_FLAG} token"));
        }
        let required =
            |slot: Option<u64>, flag: &str| slot.ok_or_else(|| format!("this mode needs {flag}"));
        let small = |value: u64, flag: &str| {
            u32::try_from(value).map_err(|_| format!("{flag} is out of range: {value}"))
        };
        let mode_name = mode.ok_or("a worker needs --mode")?;
        let unused = |slots: &[(Option<u64>, &str)]| -> Result<(), String> {
            match slots.iter().find(|(slot, _)| slot.is_some()) {
                Some((_, flag)) => Err(format!("{flag} does not apply to --mode {mode_name}")),
                None => Ok(()),
            }
        };
        let mode = match mode_name.as_str() {
            "cpu" => {
                unused(&[
                    (mib, "--mib"),
                    (count, "--count"),
                    (lifetime, "--lifetime-ms"),
                    (interval, "--interval-ms"),
                ])?;
                Mode::Cpu {
                    threads: small(required(threads, "--threads")?, "--threads")?,
                }
            }
            "memory" => {
                unused(&[
                    (threads, "--threads"),
                    (count, "--count"),
                    (lifetime, "--lifetime-ms"),
                    (interval, "--interval-ms"),
                ])?;
                Mode::Memory {
                    mib: small(required(mib, "--mib")?, "--mib")?,
                }
            }
            "threads" => {
                unused(&[
                    (mib, "--mib"),
                    (count, "--count"),
                    (lifetime, "--lifetime-ms"),
                    (interval, "--interval-ms"),
                ])?;
                Mode::Threads {
                    count: small(required(threads, "--threads")?, "--threads")?,
                }
            }
            "short-lived" => {
                unused(&[(threads, "--threads"), (mib, "--mib")])?;
                Mode::ShortLived {
                    count: small(required(count, "--count")?, "--count")?,
                    lifetime: Duration::from_millis(required(lifetime, "--lifetime-ms")?),
                    interval: Duration::from_millis(required(interval, "--interval-ms")?),
                }
            }
            "child" => {
                unused(&[
                    (threads, "--threads"),
                    (mib, "--mib"),
                    (count, "--count"),
                    (lifetime, "--lifetime-ms"),
                    (interval, "--interval-ms"),
                ])?;
                Mode::Child
            }
            other => return Err(format!("unknown --mode {other:?}")),
        };
        let spec = WorkerSpec {
            owner,
            mode,
            duration: Duration::from_millis(required(duration, "--duration-ms")?),
            fail_after: fail_after.map(Duration::from_millis),
        };
        spec.validate()?;
        Ok(spec)
    }
}

/// Sleeps `duration`, waking early once `stop` is set.
fn nap(stop: &AtomicBool, duration: Duration) {
    let step = Duration::from_millis(5);
    let mut left = duration;
    while !left.is_zero() && !stop.load(Ordering::Relaxed) {
        let now = left.min(step);
        thread::sleep(now);
        left -= now;
    }
}

/// What a running workload holds; dropped (after [`Workload::finish`]) only
/// when the process exits.
struct Workload {
    stop: Arc<AtomicBool>,
    threads: Vec<thread::JoinHandle<()>>,
    /// The memory a memory worker holds.
    _memory: Vec<u8>,
    /// A short-lived worker's children still running, which it owns.
    children: Arc<Mutex<Vec<Child>>>,
}

impl Workload {
    fn start(spec: &WorkerSpec) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let children = Arc::new(Mutex::new(Vec::new()));
        let mut threads = Vec::new();
        let mut memory = Vec::new();
        match &spec.mode {
            Mode::Cpu { threads: count } => {
                for index in 0..*count {
                    let stop = Arc::clone(&stop);
                    threads.push(
                        thread::Builder::new()
                            .name(format!("demo-cpu-{index}"))
                            .spawn(move || {
                                let mut state = u64::from(index) + 1;
                                while !stop.load(Ordering::Relaxed) {
                                    for _ in 0..10_000 {
                                        state = state
                                            .wrapping_mul(6_364_136_223_846_793_005)
                                            .wrapping_add(1_442_695_040_888_963_407);
                                    }
                                    std::hint::black_box(state);
                                }
                            })?,
                    );
                }
            }
            Mode::Memory { mib } => {
                let bytes = (*mib as usize) << 20;
                memory = vec![0_u8; bytes];
                // Touch every page, so the memory is resident rather than reserved.
                for page in (0..bytes).step_by(4096) {
                    memory[page] = 1;
                }
                std::hint::black_box(&memory);
            }
            Mode::Threads { count } => {
                for index in 0..*count {
                    let stop = Arc::clone(&stop);
                    threads.push(
                        thread::Builder::new()
                            .name(format!("demo-park-{index}"))
                            .stack_size(64 * 1024)
                            .spawn(move || {
                                while !stop.load(Ordering::Relaxed) {
                                    thread::park_timeout(Duration::from_millis(50));
                                }
                            })?,
                    );
                }
            }
            Mode::ShortLived {
                count,
                lifetime,
                interval,
            } => {
                let child = WorkerSpec {
                    owner: spec.owner.clone(),
                    mode: Mode::Child,
                    duration: *lifetime,
                    fail_after: None,
                };
                let exe = std::env::current_exe()?;
                let (count, interval) = (*count, *interval);
                let stop = Arc::clone(&stop);
                let children = Arc::clone(&children);
                threads.push(thread::Builder::new().name("demo-spawner".into()).spawn(
                    move || {
                        let reap = |children: &mut Vec<Child>| {
                            children.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
                        };
                        for _ in 0..count {
                            loop {
                                if stop.load(Ordering::Relaxed) {
                                    return;
                                }
                                let mut live = children.lock().unwrap();
                                reap(&mut live);
                                if live.len() < MAX_LIVE_CHILDREN as usize {
                                    break;
                                }
                                drop(live);
                                thread::sleep(Duration::from_millis(5));
                            }
                            // The child's stdin is its lifeline to this worker.
                            let spawned = Command::new(&exe)
                                .args(child.to_args())
                                .env(OWNER_ENV, &child.owner)
                                .stdin(Stdio::piped())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn();
                            match spawned {
                                Ok(spawned) => children.lock().unwrap().push(spawned),
                                Err(_) => return,
                            }
                            nap(&stop, interval);
                        }
                        // Reap the last children as they end.
                        while !stop.load(Ordering::Relaxed) {
                            let mut live = children.lock().unwrap();
                            reap(&mut live);
                            if live.is_empty() {
                                return;
                            }
                            drop(live);
                            thread::sleep(Duration::from_millis(5));
                        }
                    },
                )?);
            }
            Mode::Child => {}
        }
        Ok(Self {
            stop,
            threads,
            _memory: memory,
            children,
        })
    }

    /// Stops the threads and ends every child still running: each is this
    /// worker's own child, killed through its own handle and reaped.
    fn finish(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        for mut child in self.children.lock().unwrap().drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Runs one worker to its end and returns its exit status. The caller exits
/// the process with it.
pub fn run(spec: &WorkerSpec) -> i32 {
    if let Err(error) = spec.validate() {
        eprintln!("srtop-demo-worker: {error}");
        return exit::REFUSED;
    }
    let (ended, why) = mpsc::channel::<i32>();
    // The lifeline: `stop` ends the worker; end of file or an error means the
    // owner is gone.
    {
        let ended = ended.clone();
        let lifeline = thread::Builder::new()
            .name("demo-lifeline".into())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut line = String::new();
                loop {
                    line.clear();
                    match stdin.lock().read_line(&mut line) {
                        Ok(0) | Err(_) => {
                            let _ = ended.send(exit::ORPHANED);
                            return;
                        }
                        Ok(_) if line.trim() == "stop" => {
                            let _ = ended.send(exit::STOPPED);
                            return;
                        }
                        Ok(_) => {}
                    }
                }
            });
        if lifeline.is_err() {
            return exit::REFUSED;
        }
    }
    if let Some(after) = spec.fail_after {
        let ended = ended.clone();
        let _ = thread::Builder::new()
            .name("demo-failure".into())
            .spawn(move || {
                thread::sleep(after);
                let _ = ended.send(exit::FAILED);
            });
    }
    drop(ended);
    let mut workload = match Workload::start(spec) {
        Ok(workload) => workload,
        Err(error) => {
            eprintln!("srtop-demo-worker: could not start: {error}");
            return exit::REFUSED;
        }
    };
    // The hard deadline: whatever the lifeline and the workload do, this
    // thread returns by then and the process exits.
    let code = why.recv_timeout(spec.duration).unwrap_or(exit::COMPLETED);
    workload.finish();
    code
}
