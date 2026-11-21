//! PX-025: bounded demonstration workloads and repeatable latency scenarios for
//! the Process Explorer (D1 §31.4; D2 T28–T29, T34).
//!
//! * [`worker`]: the opt-in `srtop-demo-worker` binary's specification and
//!   body — bounded CPU, memory, thread and short-lived-process modes, each
//!   marked as test-owned and each ending by itself at a hard deadline, or as
//!   soon as its owner goes away.
//! * [`harness`]: the owner's side — a [`harness::DemoWorker`] cleanup handle
//!   per worker, and a sweep that finds a run's processes by its unique owner
//!   token, never by name.
//! * [`relay`]: a userspace delay line between a client and srtop's socket
//!   that adds RTT/2 in each direction, counts the bytes it carries and can
//!   cut every connection (a transport interruption). No root, `tc`, `dnctl`
//!   or `pfctl` is involved.
//! * [`wire`] and [`scenario`]: a minimal replica client and the scenarios
//!   composed from these pieces and srtop's own fixtures — remote result
//!   latency at 0/100/300/600 ms RTT, a transport interruption with resume,
//!   and a collector failure.
//!
//! Local interaction latency (typing, caret, scrolling of cached rows) is a
//! property of the native client and is measured there, through this relay,
//! by `client-macos/Tests/SRUITests/ProcessExplorerLatencyTests.swift`.
//!
//! This is a separate package from srtop: srtop's package must stay free of
//! process creation, signals and outbound sockets (PX-008), and nothing here is
//! linked into it.

pub mod harness;
pub mod relay;
pub mod scenario;
pub mod wire;
pub mod worker;
