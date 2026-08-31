//! Process termination interfaces and OS signal signaling (§27).

use crate::domain::ProcessKey;

/// Failure modes of a termination request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminateError {
    /// The PID does not denote exactly one process and must never be signalled.
    InvalidTarget(u32),
    /// The caller lacks permission to signal the target.
    PermissionDenied,
    /// The target process no longer exists.
    NoSuchProcess,
    /// Any other OS failure.
    Other(String),
}

impl std::fmt::Display for TerminateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTarget(pid) => {
                write!(f, "{pid} is not a single-process signal target")
            }
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::NoSuchProcess => write!(f, "no such process"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Converts a numeric PID into a `kill(2)` target that can only ever mean one process.
///
/// `kill(2)` gives non-positive PIDs broadcast semantics: `0` signals the caller's entire process
/// group, `-1` every process the caller may signal, and any other negative value a process group.
/// A `u32 as i32` cast can produce all three (`0` directly, and anything above `i32::MAX` by
/// wrapping negative), so the value is validated instead of cast.
pub fn signal_target_pid(pid: u32) -> Result<i32, TerminateError> {
    match i32::try_from(pid) {
        Ok(target) if target > 0 => Ok(target),
        _ => Err(TerminateError::InvalidTarget(pid)),
    }
}

/// Sends a termination signal to a numeric PID or stable process handle.
pub trait ProcessTerminator: Send + Sync {
    /// Sends `SIGTERM` to `pid`. Implementations must use a direct OS signal API: never a shell.
    fn terminate(&self, pid: u32) -> Result<(), TerminateError>;

    /// Signals `target` through a validated identity or stable process handle (§27).
    fn terminate_key(&self, target: ProcessKey) -> Result<(), TerminateError> {
        self.terminate(target.pid)
    }
}

/// `kill(2)` / `pidfd`-backed terminator. Sends `SIGTERM` directly: never through a shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct SignalTerminator;

impl ProcessTerminator for SignalTerminator {
    fn terminate(&self, pid: u32) -> Result<(), TerminateError> {
        use nix::errno::Errno;
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        // Validated before `Pid` is constructed: a process-group or broadcast target must never
        // reach `kill(2)` (§27).
        let target = signal_target_pid(pid)?;

        match kill(Pid::from_raw(target), Signal::SIGTERM) {
            Ok(()) => Ok(()),
            Err(Errno::EPERM) => Err(TerminateError::PermissionDenied),
            Err(Errno::ESRCH) => Err(TerminateError::NoSuchProcess),
            Err(errno) => Err(TerminateError::Other(errno.to_string())),
        }
    }

    fn terminate_key(&self, target: ProcessKey) -> Result<(), TerminateError> {
        #[cfg(target_os = "linux")]
        {
            linux_pidfd::terminate_pidfd(target).or_else(|err| {
                if let TerminateError::Other(_) = err {
                    self.terminate(target.pid)
                } else {
                    Err(err)
                }
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.terminate(target.pid)
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_pidfd {
    use super::*;

    pub fn terminate_pidfd(target: ProcessKey) -> Result<(), TerminateError> {
        let pid = signal_target_pid(target.pid)?;
        unsafe {
            let fd = libc::syscall(libc::SYS_pidfd_open, pid, 0);
            if fd < 0 {
                let err = std::io::Error::last_os_error();
                return match err.raw_os_error() {
                    Some(libc::ESRCH) => Err(TerminateError::NoSuchProcess),
                    Some(libc::EPERM) => Err(TerminateError::PermissionDenied),
                    _ => Err(TerminateError::Other(err.to_string())),
                };
            }

            let ret = libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd,
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
            libc::close(fd as libc::c_int);
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                return match err.raw_os_error() {
                    Some(libc::ESRCH) => Err(TerminateError::NoSuchProcess),
                    Some(libc::EPERM) => Err(TerminateError::PermissionDenied),
                    _ => Err(TerminateError::Other(err.to_string())),
                };
            }
            Ok(())
        }
    }
}
