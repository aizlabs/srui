//! `srtop-demo-worker`: one opt-in, test-owned demonstration workload
//! (PX-025). See [`srtop_demo::worker`] for its modes, its limits and how it
//! ends.
//!
//! ```text
//! SRTOP_DEMO_WORKER_OWNER=TOKEN srtop-demo-worker --srtop-test-owner TOKEN \
//!     --mode cpu|memory|threads|short-lived --duration-ms MS \
//!     [--threads N | --mib N | --count N --lifetime-ms MS --interval-ms MS] \
//!     [--fail-after-ms MS]
//! ```

use srtop_demo::worker::{exit, run, WorkerSpec, OWNER_ENV};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let owner = std::env::var(OWNER_ENV).ok();
    let spec = match WorkerSpec::parse(&args, owner.as_deref()) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!("srtop-demo-worker: {error}");
            std::process::exit(exit::REFUSED);
        }
    };
    std::process::exit(run(&spec));
}
