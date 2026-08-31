use crate::domain::{ProcessKey, ProcessRecord, ProcessSnapshot};

/// Source of operating-system process samples.
pub trait ProcessSource: Send {
    /// Returns one full enumeration of system and process state.
    fn sample(&mut self) -> ProcessSnapshot;

    /// Re-reads the start time of `pid` right now, or `None` if no such process exists.
    fn start_time_of(&mut self, pid: u32) -> Option<u64>;
}

/// Host process source backed by `sysinfo`.
pub struct SysinfoProcessSource {
    system: sysinfo::System,
}

impl Default for SysinfoProcessSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SysinfoProcessSource {
    /// Creates a source with CPU and memory tracking enabled.
    pub fn new() -> Self {
        let mut system = sysinfo::System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        Self { system }
    }
}

impl ProcessSource for SysinfoProcessSource {
    fn sample(&mut self) -> ProcessSnapshot {
        self.system.refresh_all();
        let processes = self
            .system
            .processes()
            .iter()
            .map(|(pid, process)| ProcessRecord {
                key: ProcessKey {
                    pid: pid.as_u32(),
                    start_time: process.start_time(),
                },
                name: process.name().to_string_lossy().into_owned(),
                cpu_percent: f64::from(process.cpu_usage()),
                memory_bytes: process.memory(),
                uid: process.user_id().map(|uid| **uid),
            })
            .collect();

        ProcessSnapshot {
            cpu_percent: f64::from(self.system.global_cpu_usage()),
            memory_used: self.system.used_memory(),
            memory_total: self.system.total_memory(),
            processes,
        }
    }

    fn start_time_of(&mut self, pid: u32) -> Option<u64> {
        let pid = sysinfo::Pid::from_u32(pid);
        self.system
            .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        self.system.process(pid).map(|process| process.start_time())
    }
}

/// Returns the effective user id of this process.
pub fn effective_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}
