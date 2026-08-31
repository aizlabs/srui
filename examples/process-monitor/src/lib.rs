//! Remote process monitor application logic (§5.2, §7.2, §7.3, §7.6, §7.7, §8, §12.1, §12.2, §22.7, §22.9, §23).
//!
//! # Architecture & Protocol Invariants
//!
//! - **§5.2 Semantic state, not display remoting**: the server owns process state and publishes
//!   only the semantic mutations required by an observed change. There is no frame, repaint, or
//!   pixel concept anywhere in this example.
//! - **§8 Collections**: the process list is a single collection [`Model`] with stable
//!   [`ItemId`]s, not one semantic node per process. Item identity is derived from a server-side
//!   [`ProcessKey`] (pid + start time) so PID reuse can never alias two different processes.
//! - **§12.1 Atomic transactions**: every polling sample and every accepted semantic event
//!   produces at most one all-or-nothing transaction; an empty operation list commits nothing.
//! - **§23 Incremental performance**: steady-state traffic is `SET_PROPERTY` on the two progress
//!   nodes plus `MODEL_INSERT` / `MODEL_UPDATE` / `MODEL_DELETE` for rows that actually changed.
//!   `CREATE_NODE`, `CREATE_MODEL`, and `MODEL_RESET_RANGE` never appear after initialization.
//! - **§7.7 `action_key` is data**: action keys are transported as opaque metadata and are never
//!   parsed, dispatched, or executed. Authorization uses server-owned state only.
//!
//! # Locking
//!
//! Two mutexes exist: application state and the process source. They are **never** held at the
//! same time, and neither is ever held across an `.await`. The session's internal transaction lock
//! is only ever taken *inside* the application-state lock (state → session), never the reverse,
//! and no `sysinfo` refresh or `kill(2)` call runs while a transaction is open.

pub mod diff;
pub mod domain;
pub mod monitor;
pub mod source;
pub mod state;
pub mod terminator;
pub mod testing;
pub mod ui;
pub mod wire_stats;

pub use diff::diff_visible_rows;
pub use domain::*;
pub use monitor::{KillOutcome, Monitor};
pub use source::{effective_uid, ProcessSource, SysinfoProcessSource};
pub use state::{
    ClientSelection, MonitorState, PendingState, StateData, TickPlan, MAX_CLIENT_INSTANCE_ID_LEN,
    MAX_CLIENT_SELECTIONS,
};
pub use terminator::{signal_target_pid, ProcessTerminator, SignalTerminator, TerminateError};
pub use ui::build_initial_ui;
pub use wire_stats::{measure_transaction, TransactionWireStats};
