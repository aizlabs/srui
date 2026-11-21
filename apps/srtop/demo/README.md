# srtop demonstration workloads and latency scenarios (PX-025)

Opt-in tools for showing and testing the Process Explorer under real load and real latency (D1 §31.4; D2 T34). This is a separate Cargo package from srtop on purpose: srtop's own package is audited (PX-008, `../tests/no_process_control_test.rs`) to contain no process creation, signal or outbound socket, and these tools do all three, for processes and sockets they own. Nothing here is linked into `srtop`.

```sh
cargo build --locked --manifest-path apps/srtop/Cargo.toml        # the srtop the scenarios drive
cargo test  --locked --manifest-path apps/srtop/demo/Cargo.toml   # SRTOP_BIN overrides ../target/debug/srtop
```

## `srtop-demo-worker`

```text
SRTOP_DEMO_WORKER_OWNER=TOKEN srtop-demo-worker --srtop-test-owner TOKEN \
    --mode cpu|memory|threads|short-lived --duration-ms MS \
    [--threads N | --mib N | --count N --lifetime-ms MS --interval-ms MS] [--fail-after-ms MS]
```

| Mode | What it does | Limit |
| --- | --- | --- |
| `cpu` | spins `--threads` threads | 1–4 threads |
| `memory` | allocates `--mib` MiB and touches every page | 1–256 MiB |
| `threads` | starts `--threads` parked threads | 1–64 threads |
| `short-lived` | starts `--count` children, one per `--interval-ms`, each living `--lifetime-ms`, at most 8 at once, and reaps them | 1–64 children, 1–2000 ms each, 10–1000 ms apart |

Every run lasts 1 ms to 60 s (`--duration-ms`). A value past a limit is refused (exit 2), never clamped.

- **Test-owned.** Without an owner token `srtop-demo-[a-z0-9-]…` in both `--srtop-test-owner` and `SRTOP_DEMO_WORKER_OWNER`, a worker does nothing (exit 2). The token stays in its argument vector, and in its children's, so a test finds exactly its own run's processes by token, never by name.
- **It ends by itself.** At its deadline it exits 0. Its standard input is a lifeline: `stop` ends it (exit 20), and end of file means its owner is gone (exit 21), whatever stopped the owner, SIGKILL included. A short-lived child's lifeline is its worker, so the chain unwinds the same way. `--fail-after-ms` makes it fail (exit 22) as a failing workload would, and its children end with it.
- The owner's handle is `srtop_demo::harness::DemoWorker`: dropping it kills the worker through its own unreaped child handle and reaps it, so a test that fails cleans up as it unwinds.

## `srtop-demo`

```text
srtop-demo relay --listen PATH --upstream PATH --rtt-ms MS
srtop-demo workers --duration-ms MS [--cpu-threads N] [--memory-mib N] [--threads N] [--short-lived COUNT]
srtop-demo scenarios --srtop PATH [--trials N]
```

- **`relay`** listens on `PATH` (owner-only, as the SSH bridge requires) and joins each client to `--upstream`, adding RTT/2 in each direction as a delay line: each chunk is released RTT/2 after it was read, in order, without queuing behind the others. Each direction of each connection holds at most 4 MiB read but not yet written on (`DEFAULT_IN_FLIGHT`; `DelayRelay::start_with_budget` takes 1 byte to 64 MiB): when that is full the relay stops reading, so the sender blocks just as it would on a socket to a client that stopped reading, and backpressure still reaches srtop. It prints `ready …`; on standard input `cut` cuts every live connection (`cut N`) and `traffic` prints the bytes carried each way; end of file stops it and removes its socket. To put it in the SSH path, point the bridge's subsystem at the relay's socket (`Subsystem srui srui-ssh-bridge RELAY.sock`). The RTT is at most 10 s.
- **`workers`** starts one worker per named workload under one fresh owner token and prints `owner TOKEN` and `worker MODE PID`. Each flag may be given once (at most four workers per run); a repeated one is refused (exit 2) before anything starts. If it is interrupted, its workers end with it.
- **`scenarios`** starts srtop's own fixtures in a private directory and prints one JSON report: filter and search result latency through the relay at 0/100/300/600 ms RTT (`--fake-repeated-names`), a transport interruption resumed with the runtime's `CLIENT_RESUME` at each RTT, and the scripted collector failure at 300 ms (`--fake-sequence`).

The replica client behind `scenarios` (`srtop_demo::wire`) refuses a frame whose declared length exceeds the 16 MiB §26 limit before buffering any of it.

Local interaction latency is the native client's, and is measured through this relay by `client-macos/Tests/SRUITests/ProcessExplorerLatencyTests.swift`.
