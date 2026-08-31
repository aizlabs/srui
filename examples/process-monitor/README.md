# Process Monitor Example

A real, runnable SRUI application: a remote system monitor whose CPU meter, memory meter and
process table live on the server, and whose *semantic state* — not pixels — is replicated to the
native macOS renderer (§5.2).

What it demonstrates:

- **Semantic state replication (§5.2).** The server owns process state. Each polling sample commits
  one atomic transaction (§12.1) containing only the mutations implied by what actually changed.
  There is no frame, repaint, or display-tick concept anywhere in the example (§12.2).
- **Model-backed collections (§8, §22.7).** The process list is a single collection model with
  stable `ItemId`s — not one semantic node per process. Steady-state traffic is
  `MODEL_INSERT` / `MODEL_UPDATE` / `MODEL_DELETE` for changed rows plus `SET_PROPERTY` on the two
  progress nodes. `CREATE_NODE`, `CREATE_MODEL` and `MODEL_RESET_RANGE` never appear after startup.
- **Semantic events (§7.6, §7.7).** Table selection, toggle changes, and the destructive button are
  ordinary semantic events. `action_key` values (`process.show-all`, `process.kill-selected`) travel
  as opaque metadata; they are never parsed, dispatched, or executed.
- **Server authority & Client Isolation (§27).** Filtering, selection validity, and kill authorization are decided by
  the server from its own state. Multiple connected clients maintain isolated selections and cannot signal processes on behalf of other clients. Nothing about a kill target is read from client input.

## ⚠️ Safety & Security

**"Kill Selected" sends a real `SIGTERM` to a real process on the machine running the server.**
There is no confirmation dialog and no undo — both are deliberately out of scope.

Guardrails that are always in force:

- **Denylist.** PID 0 (which would signal the caller's entire process group), PID 1 (`init` / `launchd`), and the process-monitor server's own PID (`std::process::id()`) are strictly denylisted. A denied request performs no signal operation, leaves the server running, logs a clear refusal, and returns normally.
- **Per-client selection ownership.** A selection is recorded against the `client_instance_id` that made it, and "Kill Selected" only ever resolves the activating client's own selection — never another client's, and never a shared fallback. An event that carries no `client_instance_id` cannot be attributed to an owner and is refused outright.
- **PID-reuse protection & Linux `pidfd`.** A selection is an `ItemId`. The server resolves it to the `ProcessKey(pid, start_time)` it assigned. On Linux, signalling uses `pidfd_open` and `pidfd_send_signal` for race-free process targeting. On other Unix platforms, the server re-reads the live process start time immediately before signalling via `kill(2)`. A mismatch or vanished PID is refused as stale.
- **Single-process numeric validation.** PID targets are strictly validated within `[1, i32::MAX]` before invoking system APIs. Process group (`0`, negative) and broadcast (`-1`) targets cannot be signalled.
- **Socket protection.** The Unix domain socket is created `0600` (owner-only) and guarded by an exclusive `flock(2)` lock file held for the process lifetime, plus a connect probe, before any stale predecessor is unlinked — a live endpoint can never be hijacked, and a crashed server's socket is still reclaimable.
- **The socket is the authorization boundary.** There is no per-connection authentication: anything that can connect to the endpoint can select a row and press **Kill Selected**, subject to every guardrail above. `0600` on the socket, and a private parent directory (`$XDG_RUNTIME_DIR`, or a `0700` directory you choose with `--socket`), are therefore load-bearing. Over SSH the SSH login *is* the authentication. Do not place the socket in a world-writable directory or relax its mode.
- **No shell.** Termination goes directly through `kill(2)` / `pidfd`. The example never constructs shell commands and never invokes `sh`, `bash`, `zsh`, or `system()`.
- **PID text is never trusted.** Row text, PID text, row index, labels and `action_key` sent by the client are ignored when resolving the target.

## Filtering

By default (`show_all = false`) the table lists processes owned by the effective user running the
monitor. Turning on **Show all processes** switches to the full enumeration. This filter is
authoritative server behaviour: the client never invents or applies it locally. The toggle's
authoritative `VALUE` is confirmed back to the client in the same transaction that republishes
membership. Filtered mode means exactly "owned by the effective user": a process whose owner the
platform does not report cannot be proven to be yours, so it is excluded until **Show all
processes** is enabled.

Rows are sorted by `(pid, start_time)` — deliberately *not* by CPU, which would reorder most rows
every second and force structural model churn (§23).

## Build

```bash
cargo build --manifest-path examples/process-monitor/Cargo.toml --release
```

## Run locally over a Unix socket

```bash
# defaults to $XDG_RUNTIME_DIR/srui-process-monitor.sock, else $TMPDIR/srui-process-monitor.sock
./examples/process-monitor/target/release/process-monitor

# explicit socket path plus wire accounting
./examples/process-monitor/target/release/process-monitor \
    --socket /tmp/srui-process-monitor.sock --wire-stats
```

Options:

| Flag | Meaning |
| --- | --- |
| `--socket <path>` | Unix socket to bind. Defaults to `srui-process-monitor.sock`. |
| `--wire-stats` | Log the framed byte size and operation mix of every committed transaction. |

A malformed or missing option argument is rejected with a clear error. Ownership of the endpoint is
decided by an exclusive `flock(2)` on a `<socket>.lock` file, taken before the socket is touched and
held for the process lifetime: a second instance refuses to start instead of stealing the endpoint,
and because the kernel releases the lock when the process dies — `SIGKILL` included — a crashed
server never leaves the path permanently claimed. A connect probe additionally refuses to displace a
*foreign* listener that does not participate in the lock. The lock is held across the unlink and the
bind, so two instances can never both conclude the existing socket was stale, and a path that exists
but is not a socket is an error rather than a removal candidate. On clean shutdown the socket is
removed only while the path still resolves to the endpoint this process bound, so a replacement
server's socket is left alone.
All diagnostics go to stderr, so a bridged stdout stays a pure binary protocol stream (§19.1).
`Ctrl-C` stops accepting connections, cancels connection and polling tasks, cleans up the socket,
and exits without panicking.

## Run over SSH

On the **server** host, run the monitor on the default socket path, then expose the bridge as an SSH
subsystem in `/etc/ssh/sshd_config`, pointing it at the monitor's endpoint:

```
Subsystem srui /usr/local/bin/srui-ssh-bridge --socket /run/user/1000/srui-process-monitor.sock
```

(`srui-ssh-bridge` is built from `server-rust/ssh-bridge` and forwards stdio to the socket it is
given. Its *default* is `$XDG_RUNTIME_DIR/srui-sessiond.sock`, falling back to the temp directory —
that is `srui-sessiond`'s endpoint, not this example's, so the `--socket` argument above is
required. Substitute the `$XDG_RUNTIME_DIR` of the account that runs the monitor.)

On the **client** Mac:

```bash
swift build --package-path client-macos
swift run --package-path client-macos RendererDemoApp --ssh server-host --user alice

# or, when server and client are the same machine, skip SSH entirely:
swift run --package-path client-macos RendererDemoApp --socket /tmp/srui-process-monitor.sock
```

A freshly connected client receives `SERVER_WELCOME` followed by a complete authoritative snapshot
of the tree and the process model; the example never resends a snapshot itself.

## Reading `--wire-stats`

Each committed transaction logs one line:

```
revision=9 ops=3 set_property=2 model_insert=0 model_update=1 model_delete=0 other=0 framed_bytes=3105
```

`framed_bytes` is the length of the varint length-delimited SRUI frame — the same encoding
`handle_connection` writes to the socket — **measured before SSH encryption or compression**.

Representative measurement on a 769-process Mac:

| Transaction | Operations | Framed bytes |
| --- | --- | --- |
| Initial snapshot (revision 1) | 10 × `CREATE_NODE`, `CREATE_MODEL`, `MODEL_INSERT` of 769 rows | 40 632 |
| Typical steady-state tick | 2–4 `SET_PROPERTY`, 1 batched `MODEL_UPDATE` | ~3 100 |

Bytes vary with real activity: on a busy machine many rows legitimately change CPU each second. The
invariant is proportionality to changed state, not a fixed byte count. A tick in which nothing
changed commits no transaction at all.

## Manual verification

1. Start the monitor with `--wire-stats` and attach the macOS client. The CPU and memory progress
   controls update about once per second.
2. On the server host, start a harmless throwaway process: `sleep 600 &`.
3. Confirm the row appears within one or two polling ticks.
4. Select the row and press **Kill Selected**.
5. Confirm the log reports `SIGTERM delivered to pid <pid>` and the real process exits.
6. Confirm the row disappears within one or two ticks (published as a `MODEL_DELETE`).
7. Toggle **Show all processes** and confirm membership changes through server updates.
8. Select PID 1 (visible with `show_all` on) and press **Kill Selected**: the log reports
   `kill refused: pid 1 is denylisted` and `init`/`launchd` keeps running.
9. Select the process-monitor server itself and press **Kill Selected**: same refusal, server stays
   up.
10. Watch `--wire-stats` during a quiet period: no tick contains `CREATE_NODE`, `CREATE_MODEL`,
    `MODEL_RESET_RANGE`, a replacement tree, or a full model resend.

## Tests

```bash
cargo test --manifest-path examples/process-monitor/Cargo.toml
```

Every test drives the monitor through deterministic fakes (`srui_example_process_monitor::testing`).
No test enumerates or signals arbitrary real host processes.

## Limitations

- No model virtualization, range fetching, or viewport-driven loading: the whole visible set is
  published.
- No client-side sorting or filtering; the server decides membership and order.
- No new reconnect/resume logic — the existing session, journal and bootstrap paths own that.
- No new coalescing or backpressure behaviour.
- No terminal or PTY embedding.
- No confirmation dialog and no undo for the destructive action.
- Unix-like server only (`kill(2)`, Unix domain sockets, effective-uid filtering).
