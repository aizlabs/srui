//! The owner's side of a demonstration worker (PX-025).
//!
//! A [`DemoWorker`] is the cleanup handle of one worker this process started:
//! it holds the worker's [`Child`] and the write end of its lifeline. Dropping
//! it — after success, after a failed assertion unwinds, or when the owning
//! process ends for any reason — ends the worker: an orderly drop kills the
//! worker through its own handle and reaps it, and an owner that dies without
//! running any code (SIGKILL) closes the lifeline, which ends the worker by
//! itself.
//!
//! Nothing here signals a process by name. The only process a handle signals
//! is its own unreaped child, whose PID cannot have been reused, and only
//! while that PID carries the run's owner token; [`processes_with_owner`]
//! finds a run's processes by that unique token in their argument vectors.

use crate::worker::{WorkerSpec, OWNER_ENV, OWNER_FLAG, OWNER_PREFIX};
use std::{
    io::{self, Write},
    path::Path,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A fresh owner token, unique to this process and call.
pub fn fresh_owner() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!(
        "{OWNER_PREFIX}{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The cleanup handle of one running worker.
pub struct DemoWorker {
    child: Child,
    lifeline: Option<ChildStdin>,
    owner: String,
    status: Option<ExitStatus>,
}

impl DemoWorker {
    /// Starts `spec` with the worker binary `exe`. An invalid specification
    /// is refused here, before anything is started.
    pub fn spawn(exe: &Path, spec: &WorkerSpec) -> io::Result<Self> {
        spec.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut child = Command::new(exe)
            .args(spec.to_args())
            .env(OWNER_ENV, &spec.owner)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let lifeline = child.stdin.take();
        Ok(Self {
            child,
            lifeline,
            owner: spec.owner.clone(),
            status: None,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The worker's exit status, if it has ended.
    pub fn try_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self.child.try_wait()?;
        }
        Ok(self.status)
    }

    /// Waits up to `timeout` for the worker to end by itself.
    pub fn wait_timeout(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_status()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Asks the worker to stop and waits up to `grace`; a worker that has not
    /// ended by then is killed through its handle. Returns its exit status.
    pub fn stop(&mut self, grace: Duration) -> io::Result<ExitStatus> {
        if let Some(lifeline) = self.lifeline.as_mut() {
            // A worker that already ended has closed its end; that is fine.
            let _ = lifeline.write_all(b"stop\n").and_then(|_| lifeline.flush());
        }
        if let Some(status) = self.wait_timeout(grace)? {
            return Ok(status);
        }
        self.kill()
    }

    /// Closes the lifeline without stopping the worker, as an owner that
    /// vanished does.
    pub fn cut_lifeline(&mut self) {
        self.lifeline = None;
    }

    /// Ends the worker and reaps it. The lifeline is closed first, so the
    /// worker ends by itself even unsignalled; it is signalled (SIGKILL) only
    /// through its own unreaped handle, whose PID cannot have been reused, and
    /// only while that PID still carries this run's owner token.
    pub fn kill(&mut self) -> io::Result<ExitStatus> {
        self.lifeline = None;
        if let Some(status) = self.try_status()? {
            return Ok(status);
        }
        let pid = self.pid();
        let marked = live_processes_with_owner(&self.owner)
            .map(|found| found.iter().any(|process| process.pid == pid))
            .unwrap_or(false);
        if marked {
            let _ = self.child.kill();
        }
        let status = self.child.wait()?;
        self.status = Some(status);
        Ok(status)
    }
}

impl Drop for DemoWorker {
    fn drop(&mut self) {
        self.lifeline = None;
        let _ = self.kill();
    }
}

/// One process found by [`processes_with_owner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedProcess {
    pub pid: u32,
    /// The kernel's one-letter state (`R`, `S`, `Z`, …).
    pub state: char,
}

impl OwnedProcess {
    /// A zombie has ended; only its exit status is left for its parent.
    pub fn is_live(&self) -> bool {
        self.state != 'Z'
    }
}

/// Every process whose argument vector carries `OWNER_FLAG owner` exactly.
/// The token is unique per run, so this finds that run's processes and no
/// other: no name is matched.
pub fn processes_with_owner(owner: &str) -> io::Result<Vec<OwnedProcess>> {
    let carries = |args: &[&str]| {
        args.windows(2)
            .any(|pair| pair[0] == OWNER_FLAG && pair[1] == owner)
    };
    let mut found = Vec::new();
    #[cfg(target_os = "linux")]
    {
        for entry in std::fs::read_dir("/proc")? {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
            else {
                continue;
            };
            // A process can end between the listing and these reads.
            let (Ok(cmdline), Ok(stat)) = (
                std::fs::read(entry.path().join("cmdline")),
                std::fs::read_to_string(entry.path().join("stat")),
            ) else {
                continue;
            };
            let text = String::from_utf8_lossy(&cmdline);
            let args: Vec<&str> = text.split('\0').collect();
            if !carries(&args) {
                continue;
            }
            let state = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.trim_start().chars().next())
                .unwrap_or('?');
            found.push(OwnedProcess { pid, state });
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // `ps` prints each process's full argument vector space-separated; the
        // token has no spaces, so the pair is still two adjacent words. A
        // zombie has no arguments left, so it is found only while it has them.
        let output = Command::new("/bin/ps")
            .args(["-ww", "-A", "-o", "pid=,stat=,command="])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut words = line.split_whitespace();
            let (Some(pid), Some(stat)) = (words.next(), words.next()) else {
                continue;
            };
            let args: Vec<&str> = words.collect();
            if !carries(&args) {
                continue;
            }
            if let Ok(pid) = pid.parse() {
                found.push(OwnedProcess {
                    pid,
                    state: stat.chars().next().unwrap_or('?'),
                });
            }
        }
    }
    Ok(found)
}

/// The live processes carrying `owner`.
pub fn live_processes_with_owner(owner: &str) -> io::Result<Vec<OwnedProcess>> {
    Ok(processes_with_owner(owner)?
        .into_iter()
        .filter(OwnedProcess::is_live)
        .collect())
}

/// Waits up to `timeout` until no live process carries `owner`; returns the
/// ones still live at the deadline.
pub fn wait_until_none_live(owner: &str, timeout: Duration) -> io::Result<Vec<OwnedProcess>> {
    let deadline = Instant::now() + timeout;
    loop {
        let live = live_processes_with_owner(owner)?;
        if live.is_empty() || Instant::now() >= deadline {
            return Ok(live);
        }
        thread::sleep(Duration::from_millis(20));
    }
}
