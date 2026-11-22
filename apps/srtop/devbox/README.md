# srtop Linux dev box

A throwaway Linux container that runs `srtop` against a real `/proc` and publishes
it through the ordinary `srui-ssh-bridge` SSH subsystem, so the unmodified macOS
client can be pointed at live process data from a Mac. It exists for eyes-on
checks that no automated test can make — does the table flicker, does scroll
position survive a refresh, does a row appear when a process really starts.

It is not part of any test suite and nothing in CI runs it. One script turns it
into a repeatable check: `apps/srtop/devbox/r0-scenario.sh` (PX-008, the R0 gate)
starts the box, runs the native `ProcessExplorerDevboxScenarioTests` against it —
an empty client window, a worker started inside the box, its row appearing,
warming up and then sampled, the worker ending on its own and its row going —
and stops the box again (`SRTOP_R0_KEEP_BOX=1` keeps it), also when `up` or the
scenario fails or is interrupted. It refuses an image built from a revision that
differs from the one under test in anything the image contains, and names the
`devbox.sh build` to run. With `PX008_EVIDENCE_DIR` set it also writes the client
window's image and an operation/byte trace there. Without the box's coordinates in
the environment that suite reports itself skipped, never passed.

The box is only useful against a revision whose `srtop` polls its source, so this
branch is stacked on PX-004 and `SRTOP_DEVBOX_REV` defaults to `HEAD`. Exporting
an older revision produces a server that takes one snapshot and never refreshes:
`up` refuses to pretend otherwise and reports srtop's own argument error.

## Turn it on

```sh
docker info >/dev/null                 # Docker Desktop must be running
apps/srtop/devbox/devbox.sh up         # builds the image on first use (~5-10 min), then starts it
apps/srtop/devbox/devbox.sh client     # builds and launches the macOS client against it
```

`up` is idempotent: it replaces any previous container of the same name and
rewrites `known_hosts`, because the container generates fresh host keys each run.
The image is built once and reused; force a rebuild after changing the server
sources with `apps/srtop/devbox/devbox.sh build`.

## Drive it

```sh
apps/srtop/devbox/devbox.sh spawn 30      # one `sleep 30` inside the box
apps/srtop/devbox/devbox.sh spawn 25 20   # twenty of them, to watch batched inserts
apps/srtop/devbox/devbox.sh ssh 'pkill -f "sleep 30"'
apps/srtop/devbox/devbox.sh status        # container state plus its process list
apps/srtop/devbox/devbox.sh logs          # sshd and startup output
```

A spawned process should appear within two sampling intervals and disappear
within two of its exit. Rows that did not change keep their item identity, so
the table must not rebuild or jump.

Two commands exist for the R1 checklist (PX-026-G01). Each signals only a PID
whose name `ps -p PID -o comm=` has confirmed inside the box:

```sh
apps/srtop/devbox/devbox.sh drop-bridges    # kill each client's srui-ssh-bridge: a disconnect; the session lives on
apps/srtop/devbox/devbox.sh restart-srtop   # SIGTERM srtop, start a new one on the same socket: a replacement
```

PX-027's demonstration workload is one command. It runs PX-025's bounded, test-owned workers inside
the box: one CPU thread, 64 MiB resident, 16 parked threads, and a short-lived process every
few hundred milliseconds. Each ends by itself at the deadline, which is at most 60 s:

```sh
apps/srtop/devbox/devbox.sh workers 60
```

`apps/srtop/r1/showcase-measure.sh` measures srtop against htop on this box, with the same
workloads; see [../r1/README.md](../r1/README.md).

`apps/srtop/r1/live-checks.sh` is the R1 gate's scripted run against this box. It
covers protocol profiles and core versions, host keys, authentication,
reconnect, replacement, the package's controls and the privacy of command
lines; see [../r1/README.md](../r1/README.md).

## Turn it off

```sh
apps/srtop/devbox/devbox.sh down       # stop and remove the container
```

Quit the client window separately; it is an ordinary macOS app. To reclaim the
image and state as well:

```sh
docker image rm srtop-devbox:latest
rm -rf "${SRTOP_DEVBOX_STATE:-$HOME/.codex/srtop-devbox}"
```

## Knobs

| Variable | Default | Meaning |
| --- | --- | --- |
| `SRTOP_DEVBOX_STATE` | `$HOME/.codex/srtop-devbox` | keypair, `known_hosts`, build context — outside every checkout |
| `SRTOP_DEVBOX_NAME` | `srtop-devbox` | container name |
| `SRTOP_DEVBOX_IMAGE` | `srtop-devbox:latest` | image tag |
| `SRTOP_DEVBOX_PORT` | `2222` | loopback-only host port forwarded to the container's sshd |
| `SRTOP_DEVBOX_REV` | `HEAD` | revision exported into the build context; must be PX-004 or later |
| `SRTOP_ARGS` | `--live-source --refresh-interval-ms 1000` | srtop flags |

## How it is wired

- Both base images are pinned by digest in the Dockerfile (PX-026-G01): the build
  stage is `rust:1-bookworm` with rustc 1.99.1, and the runtime is
  `debian:bookworm-slim` with Debian 12.15. The apt packages installed on top of
  them (openssh-server, procps) are not pinned. `live-checks.sh` records their
  versions. PX-027:
  - every binary is a release build;
  - the image also carries `srtop-demo` and `srtop-demo-worker` (PX-025's bounded
    workers);
  - it carries htop for the matched comparison, pinned to bookworm's `3.2.2-2`.
- The build context is a `git archive` of `SRTOP_DEVBOX_REV`, never the working
  tree, so no `.git`, no `target/`, and no key material reaches the image. The
  image is labelled `org.srui.revision` with that commit; `r0-scenario.sh`
  compares it with the revision under test and prints it, so a run's evidence
  names the srtop it exercised.
- `srtop` runs as the unprivileged `srui` account on a `0700` directory; it
  refuses a socket directory that is not private to a non-root user.
- sshd accepts public keys only, for that one account, and exposes exactly one
  subsystem: `srui` → `srui-ssh-bridge --socket /home/srui/run/srtop.sock`. The
  port is published as `127.0.0.1:$SRTOP_DEVBOX_PORT`, so the throwaway daemon is
  never reachable from the LAN, Wi-Fi or a VPN interface.
- The container has its own PID namespace, so the process list is the
  container's own — a short list dominated by `sshd`, `srtop` and whatever you
  spawn. For a busy host, point the client at a real Linux machine instead.
- The client is the stock `RendererDemoApp` with `--ssh`; no app-specific client
  code path exists or is needed.
