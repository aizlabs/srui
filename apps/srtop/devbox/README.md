# srtop Linux dev box

A throwaway Linux container that runs `srtop` against a real `/proc` and publishes
it through the ordinary `srui-ssh-bridge` SSH subsystem, so the unmodified macOS
client can be pointed at live process data from a Mac. It exists for eyes-on
checks that no automated test can make — does the table flicker, does scroll
position survive a refresh, does a row appear when a process really starts.

It is not part of any test suite and nothing in CI runs it.

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
| `SRTOP_DEVBOX_PORT` | `2222` | host port forwarded to the container's sshd |
| `SRTOP_DEVBOX_REV` | `HEAD` | revision exported into the build context |
| `SRTOP_ARGS` | `--live-source --refresh-interval-ms 1000` | srtop flags; use `--live-source` alone before PX-004 |

## How it is wired

- The build context is a `git archive` of `SRTOP_DEVBOX_REV`, never the working
  tree, so no `.git`, no `target/`, and no key material reaches the image.
- `srtop` runs as the unprivileged `srui` account on a `0700` directory; it
  refuses a socket directory that is not private to a non-root user.
- sshd accepts public keys only, for that one account, and exposes exactly one
  subsystem: `srui` → `srui-ssh-bridge --socket /home/srui/run/srtop.sock`.
- The container has its own PID namespace, so the process list is the
  container's own — a short list dominated by `sshd`, `srtop` and whatever you
  spawn. For a busy host, point the client at a real Linux machine instead.
- The client is the stock `RendererDemoApp` with `--ssh`; no app-specific client
  code path exists or is needed.
