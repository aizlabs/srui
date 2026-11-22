# SRUI Process Explorer R1 package

PX-026-G01 packages the read-only Process Explorer for R1: a Linux host running `srtop` behind the
existing `srui-ssh-bridge` SSH subsystem and its same-user Unix socket, and the generic macOS client.
The tested Linux environment is the devbox container ([../devbox/README.md](../devbox/README.md)).
Release notes, the tested envelope and the known gaps are in
[R1.md](../srui-process-explorer-plan/docs/process-explorer/releases/R1.md).

R1 is distributed as source. You build it yourself with the pinned toolchain. Nothing is signed
beyond the ad-hoc signature the Swift linker applies, and nothing is notarized. No Apple account is
needed, and signed or notarized distribution is out of scope. No step needs `sudo`, and nothing
installs a privileged helper.

## Pinned environment

| Part | Tested with | Required |
| --- | --- | --- |
| macOS | 26.4.1 (25E253), arm64 | macOS 14 or later (`Package.swift`: `.macOS(.v14)`) |
| Swift toolchain | Xcode 26.2 (17C52), Apple Swift 6.2.3 (swiftlang-6.2.3.3.21) | Swift tools 6.0 or later |
| Client dependency | swift-protobuf 1.38.1 (`55d7a1cc`, `client-macos/Package.resolved`) | the lock file as committed |
| Client SSH | OpenSSH_10.2p1, LibreSSL 3.3.6 (`/usr/bin/ssh`) | the system `ssh` |
| Docker | Docker Desktop, engine 28.3.2, client 29.0.2 | Docker Desktop, for the devbox |
| Linux host | the devbox: Debian GNU/Linux 12 (bookworm) 12.15, `debian:bookworm-slim@sha256:734a72b3…` | — |
| Linux kernel (as the container sees it) | 6.10.14-linuxkit aarch64 | — |
| Linux sshd | openssh-server 1:9.2p1-2+deb12u10, procps 2:4.0.2-3 (installed by apt at image build; not pinned) | — |
| Server toolchain | rustc 1.99.1 (58cf8f9c3), cargo 1.99.1, `rust:1-bookworm@sha256:e6f2cff3…` | as pinned in `devbox/Dockerfile` |
| Server dependencies | `apps/srtop/Cargo.lock`, `server-rust/Cargo.lock`, built `--locked` | the lock files as committed |
| Rust on the Mac | not needed for the R1 path; rustc/cargo 1.98.0 was present on the test Mac | only for `run.sh fake` and the Rust test suites |

The Linux binaries the devbox image ran, built from `7d283609` (the merge of PX-026-G02), are recorded by
`live-checks.sh` in each run's `environment.txt`, with their SHA-256 digests.

## Build and install the macOS client

The client is the generic `RendererDemoApp`. Nothing in it is specific to the Process Explorer.

```sh
apps/srtop/r1/srui-r1-install.sh install     # swift build -c release, then install the result
~/.local/share/srui-r1/bin/srui-client       # the Connections window
```

`srui-r1-install.sh` keeps everything under `~/.local/share/srui-r1` (or `SRUI_R1_PREFIX`, which
must be inside your home directory):

- `releases/<revision>-<digest>/`: one release, holding the binary and a `MANIFEST`. The manifest
  records the revision, the SHA-256 digest, the signature kind, the Swift version and the install
  time.
- `current` and `previous`: links to releases. At most these two releases are kept.
- `bin/srui-client`: a link to the current binary.

It refuses to run as root. It refuses a prefix outside your home directory. It refuses a
non-empty directory it did not create.

## Update, roll back, uninstall

```sh
git pull && apps/srtop/r1/srui-r1-install.sh update   # build and install; the old one becomes previous
apps/srtop/r1/srui-r1-install.sh rollback             # swap current and previous
apps/srtop/r1/srui-r1-install.sh status
apps/srtop/r1/srui-r1-install.sh uninstall            # remove what it installed
apps/srtop/r1/srui-r1-install.sh uninstall --purge-settings   # and forget saved connections
```

The uninstall removes only the files it wrote: release binaries and manifests, the links, and its
marker. It then removes the directories only if they are empty, so anything else you put in the
prefix stays. `--purge-settings` removes exactly `~/Library/Application Support/SRUI/saved-connections.json`
and its `.unreadable` backup, and the directory only if nothing else is in it. Neither option
touches `~/.ssh`.

To remove the devbox as well, run `apps/srtop/devbox/devbox.sh down`, then
`docker image rm srtop-devbox:latest`, then `rm -rf ~/.codex/srtop-devbox`. The last one holds the
box's own throwaway keypair.

## The Linux side

The tested host is the devbox. `devbox.sh build` exports the committed revision with
`git archive` and builds srtop and the bridge `--locked` in the pinned Rust image. `devbox.sh up`
then runs srtop as the unprivileged `srui` account and serves the `srui` subsystem on
`127.0.0.1:2222`, accepting public keys only.

On another Linux host the same steps apply by hand, but that host is not part of R1's tested
environment:

1. As your own user, run `cargo build --locked --release --manifest-path apps/srtop/Cargo.toml`
   and `cargo build --locked --release --manifest-path server-rust/Cargo.toml -p srui-ssh-bridge`.
2. Start `srtop --socket <dir>/srtop.sock --live-source`, where `<dir>` is a directory only you
   can enter.
3. Have the host's administrator add one line, `Subsystem srui <path>/srui-ssh-bridge --socket <dir>/srtop.sock`,
   to `sshd_config`. [../devbox/sshd_config](../devbox/sshd_config) is a complete example. This
   `sshd_config` line is the only step that needs an administrator. Neither srtop nor the bridge
   ever runs as root, and both refuse to.

## Connect

- **The Connections window.** Run `srui-client` with no arguments, then use Add Connection. Host
  keys are checked strictly against your own known_hosts, and the client never prompts. An unknown
  or changed key, or a key the host does not accept, ends in a blocking "Connection Failed" alert
  that quotes `ssh` and offers no way to trust the key.
- **Directly.** Run
  `srui-client --ssh HOST --port PORT --user USER --subsystem srui [--identity KEY] [--known-hosts FILE]`.
  `apps/srtop/devbox/devbox.sh client` does this against the devbox with the box's own key and
  known_hosts.

## Checks

| Script | What it proves | Needs |
| --- | --- | --- |
| `selftest.sh` | The privacy audit's self-test. The install script's refusals, update, retention, rollback and precise uninstall. The walkthrough's root refusal, its sandboxed dry run (no write outside the throwaway home), its self-validating result and its FAIL propagation. | nothing (runs in `apps/srtop/test.sh`) |
| `live-checks.sh` | Against the devbox: compatible and incompatible profiles and core versions, unknown and changed host keys, an unauthorized key, reconnect, server replacement, no process-changing action, labels and keyboard, a command-line secret that reaches no log, and a saved-connections file written by the real client store. Records the exact environment. `theServerRefusesAnIncompatibleClientWithItsReason` needs PX-026-G02's server and fails before it. | Docker, the devbox |
| `privacy-audit.sh PATH...` | No key material, token, password or private path in the given files or directories. `--self-test` plants one secret of each class. Nothing unread passes: a missing path, an unreadable file or directory entry (including a read error such as `/proc/1/mem`'s), a FIFO, device, socket or dangling symbolic link, and `--file DIR` each fail with exit 2 and an `ERROR` line naming the path. | nothing |
| `clean-account-walkthrough.sh` | The whole install, connect, use, update, rollback, uninstall cycle in a fresh account, with one self-validating result file. It refuses, before any write, an account that already holds an R1 install, saved connections, devbox state or a `srtop-devbox` container, and stops after a failed preflight or prerequisites step. See [CLEAN_ACCOUNT_WALKTHROUGH.md](CLEAN_ACCOUNT_WALKTHROUGH.md). | a fresh macOS account, Docker |
| `checklist-session.sh` | Sets up [voiceover-keyboard-checklist.md](voiceover-keyboard-checklist.md), the manual VoiceOver and keyboard check. It edits `~/.ssh/config` only through `ssh-config-alias.sh`, which refuses a symbolic-link config or `~/.ssh` and any `Host` or `Match` line already naming `srtop-devbox`, and restores the file byte for byte, with its mode, when the session ends. | Docker |

Run the live checks under `caffeinate -u -d`, so the display stays awake for the native tests:
`caffeinate -u -d apps/srtop/r1/live-checks.sh`.
