//! Trusted server-side terminal spawn specification (§21).

use std::collections::BTreeMap;
use std::path::PathBuf;

use srui_protocol::{MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS};

/// Production default ring capacity: 1 MiB per stream.
pub const DEFAULT_RING_CAPACITY: usize = 1024 * 1024;

/// Default `TERM` value when the spec does not override it.
pub const DEFAULT_TERM: &str = "xterm-256color";

/// Trusted server-side configuration for one PTY stream.
///
/// The client cannot choose the executable or environment. Callers supply values
/// from deployment configuration.
#[derive(Debug, Clone)]
pub struct TerminalSpec {
    /// Absolute or PATH-resolved executable.
    pub executable: PathBuf,
    /// Argument vector, not including `argv[0]` duplication unless the caller wants it.
    pub args: Vec<String>,
    /// Optional working directory.
    pub working_directory: Option<PathBuf>,
    /// Extra environment entries. `TERM` defaults to [`DEFAULT_TERM`] when absent.
    pub environment: BTreeMap<String, String>,
    /// Initial columns. Must be in `1..=MAX_TERMINAL_COLUMNS`.
    pub columns: u32,
    /// Initial rows. Must be in `1..=MAX_TERMINAL_ROWS`.
    pub rows: u32,
    /// Bounded output-ring capacity in bytes. Production default is 1 MiB;
    /// tests may use a few dozen bytes.
    pub ring_capacity: usize,
}

impl Default for TerminalSpec {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("/bin/sh"),
            args: vec!["-i".to_string()],
            working_directory: None,
            environment: BTreeMap::new(),
            columns: 80,
            rows: 24,
            ring_capacity: DEFAULT_RING_CAPACITY,
        }
    }
}

impl TerminalSpec {
    /// Interactive `/bin/sh` with `TERM=xterm-256color`.
    #[must_use]
    pub fn interactive_shell() -> Self {
        Self::default()
    }

    /// Returns the environment with `TERM` filled in when the caller omitted it.
    #[must_use]
    pub fn environment_with_defaults(&self) -> BTreeMap<String, String> {
        let mut env = self.environment.clone();
        env.entry("TERM".to_string())
            .or_insert_with(|| DEFAULT_TERM.to_string());
        env
    }

    /// Validates dimensions and ring capacity before spawn.
    pub fn validate(&self) -> Result<(), String> {
        if self.executable.as_os_str().is_empty() {
            return Err("terminal executable must be non-empty".to_string());
        }
        if self.columns == 0 || self.columns > MAX_TERMINAL_COLUMNS {
            return Err(format!(
                "terminal columns {} is outside 1..={}",
                self.columns, MAX_TERMINAL_COLUMNS
            ));
        }
        if self.rows == 0 || self.rows > MAX_TERMINAL_ROWS {
            return Err(format!(
                "terminal rows {} is outside 1..={}",
                self.rows, MAX_TERMINAL_ROWS
            ));
        }
        if self.ring_capacity == 0 {
            return Err("terminal ring capacity must be positive".to_string());
        }
        Ok(())
    }
}
