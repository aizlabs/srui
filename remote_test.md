# Remote Ubuntu backend with the local macOS UI

This guide tests Task 38's connection manager using the process-monitor example: the backend runs on Ubuntu and its native UI renders on the Mac over SSH.

Commands use the real remote account from this setup, `alex`, with home directory `/home/alex`. Replace `YOUR_SERVER_IP` with the Ubuntu machine's reachable hostname or IP. SSH port 22 is assumed; for another port, add `-p PORT` to SSH commands and enter that port in the UI.

```text
Mac RendererDemoApp -> SSH subsystem srui (user alex)
  -> Ubuntu srui-ssh-bridge
  -> /home/alex/.srui/run/monitor.sock
  -> Ubuntu process-monitor
```

The monitor hosts its own SRUI session. No separate `srui-sessiond` is needed for this example. Only SSH needs network access; the backend listens on a private Unix socket.

## 1. Install Ubuntu prerequisites

Run on Ubuntu:

```bash
sudo apt update
sudo apt install -y build-essential curl git pkg-config openssh-server ca-certificates
```

Install current stable Rust through rustup if Rust is not already installed. Ubuntu's packaged Rust may be too old for the dependencies:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y
. "$HOME/.cargo/env"
rustc --version
cargo --version
```

For an existing rustup installation, use `rustup update stable`. Ubuntu does not need Swift. The Rust protocol build supplies its own `protoc`.

Check your account:

```bash
whoami
printf '%s\n' "$HOME"
```

Expected: `alex` and `/home/alex`. The backend and bridge must run as the same non-root account; both reject root execution.

## 2. Get matching code on both machines

Both machines need a revision containing Task 38, preferably the exact same commit. If a machine has no checkout:

```bash
mkdir -p ~/github
cd ~/github
git clone https://github.com/aizlabs/srui.git
```

On each machine, inspect the existing checkout and fetch:

```bash
cd ~/github/srui
git status --short --branch
git branch --show-current
git worktree list
git fetch origin
```

Preserve existing changes and other tasks. Create a dedicated test branch/worktree on each machine:

```bash
git worktree add -b codex/remote-test-local ../srui-remote-test origin/main
cd ../srui-remote-test
git rev-parse HEAD
```

Compare the printed commit IDs. If they differ, select the same commit instead of `origin/main` when creating the worktrees. For an unmerged Task 38 implementation, use its fetched branch or commit. Choose unused branch/directory names if these names already exist.

All builds below run in `~/github/srui-remote-test`. This also keeps generated build artifacts out of the shared main checkout.

## 3. Build on Ubuntu

Run as `alex`:

```bash
cd ~/github/srui-remote-test
cargo build --manifest-path server-rust/Cargo.toml -p srui-ssh-bridge --release
cargo build --manifest-path examples/process-monitor/Cargo.toml --release
sudo install -m 755 server-rust/target/release/srui-ssh-bridge /usr/local/bin/srui-ssh-bridge
```

The builds use different output directories. The bridge is installed at `/usr/local/bin/srui-ssh-bridge`; the monitor remains at `examples/process-monitor/target/release/process-monitor`.

## 4. Configure Ubuntu SSH

Open the configuration:

```bash
sudo nano /etc/ssh/sshd_config
```

Add this global line **before any `Match` section**. If a `Subsystem srui` directive already exists, replace it rather than adding a duplicate:

```text
Subsystem srui /usr/local/bin/srui-ssh-bridge --socket /home/alex/.srui/run/monitor.sock
```

Use the literal absolute path, not `$HOME` or `~`. The bridge's default socket points to `srui-sessiond`, so the explicit monitor socket is required.

Validate and activate the configuration, keeping your current administrative SSH session open:

```bash
sudo /usr/sbin/sshd -t
sudo systemctl enable --now ssh
sudo systemctl reload ssh
sudo systemctl status ssh --no-pager
```

Successful validation prints nothing. Status should show `active (running)`.

### Fix: missing privilege separation directory

This setup encountered:

```text
Missing privilege separation directory: /run/sshd
```

Fix it and retry:

```bash
sudo install -d -o root -g root -m 0755 /run/sshd
sudo /usr/sbin/sshd -t
sudo systemctl enable --now ssh
sudo systemctl reload ssh
sudo systemctl status ssh --no-pager
```

### Verify the subsystem configuration

```bash
sudo /usr/sbin/sshd -T | grep '^subsystem'
```

Expected among the output:

```text
subsystem srui /usr/local/bin/srui-ssh-bridge --socket /home/alex/.srui/run/monitor.sock
```

This checks configuration on disk. Reloading successfully and testing from the Mac below verifies that the running server accepts the subsystem.

## 5. Start the remote backend

On Ubuntu, **without sudo**:

```bash
cd ~/github/srui-remote-test
./examples/process-monitor/target/release/process-monitor \
  --socket /home/alex/.srui/run/monitor.sock --wire-stats
```

Leave this terminal running. The backend logs the listening socket and initialization, then transaction statistics. It creates missing private socket directories. The final directory must be owned by `alex` with mode `0700`; the socket is `0600`. Do not loosen permissions to bypass ownership errors.

For persistence after closing the administrative terminal, optionally start a tmux session first:

```bash
sudo apt install -y tmux
tmux new -s srui-monitor
```

Run the backend inside tmux. Detach with **Ctrl+B**, then **D**; return with `tmux attach -t srui-monitor`. Run only one backend on this socket. Stop it with **Ctrl+C** in its terminal.

## 6. Verify SSH from the Mac

Establish ordinary SSH access:

```bash
ssh alex@YOUR_SERVER_IP
```

Verify the host fingerprint before accepting a new key. Run `exit` to return to the Mac, then test the noninteractive authentication used by the app:

```bash
ssh -o BatchMode=yes -o StrictHostKeyChecking=yes alex@YOUR_SERVER_IP true
```

This must succeed without a password or key-passphrase prompt. The app uses `/usr/bin/ssh`, existing SSH configuration, the agent, and known hosts. It does not collect passwords or private keys. If necessary, configure your existing key/agent authentication first. For example, `ssh-add ~/.ssh/id_ed25519` loads an existing key at that path. Custom identity files or host settings can be placed in `~/.ssh/config`.

With the backend running, test the subsystem:

```bash
ssh -T -s alex@YOUR_SERVER_IP srui
```

A successful probe normally logs bridge startup and connection to `/home/alex/.srui/run/monitor.sock`, then waits for protocol input. Press **Ctrl+C** to exit. The server may close an idle probe after its handshake timeout. This checks the SSH/bridge path, not the complete SRUI handshake or rendering.

## 7. Build and launch on the Mac

Use macOS 14 or later and a Swift toolchain compatible with the package's Swift 6.0 tools declaration:

```bash
xcode-select -p
swift --version
```

If developer tools are missing, use `xcode-select --install`. Ensure your selected Xcode/toolchain supplies Swift 6 or later.

Build current source, then launch that exact executable:

```bash
cd ~/github/srui-remote-test
swift build --package-path client-macos --product RendererDemoApp
./client-macos/.build/debug/RendererDemoApp
```

Leave the terminal running. Alternatively, build and launch together:

```bash
swift run --package-path client-macos RendererDemoApp
```

**Pass no application arguments.** This opens **SRUI Connections**. `--ssh`, `--socket`, and `--tcp` bypass the connection manager, while `--demo` runs the local canned renderer demo.

Click **Add Connection** and enter:

| Field | Value |
| --- | --- |
| Label | Ubuntu |
| Host | Same hostname, IP, or SSH alias used in the successful SSH test |
| User | `alex` |
| Port | `22`, or your configured SSH port |

Click **Connect**. Expect a native window showing the remote process table and CPU/memory updates. The saved entry should show `connected`, a session ID, and a revision. Its primary action becomes **Open** while connected.

## 8. Manual acceptance checks

### Confirm the data is remote

In another Ubuntu terminal:

```bash
sleep 600 &
```

Look for the new process in the Mac UI after one or two polling intervals. Toggle **Show all processes** to exercise server-side filtering.

**Kill Selected sends a real SIGTERM on Ubuntu, without confirmation or undo.** Use only the disposable `sleep` process to test it. The backend should log delivery and the row should disappear.

### Saved entry and cold reconnect

1. Keep the remote backend running.
2. Quit the Mac app normally, then relaunch it.
3. Connect through the saved entry.
4. Confirm that the current remote UI returns.

Saved entries live at `~/Library/Application Support/SRUI/saved-connections.json` on the Mac. The persisted revision is display metadata, not a durable replica checkpoint. Cold reconnect uses a fresh client identity and revision 0 to recover authoritative state.

### Warm reconnect

1. Leave both apps running.
2. Temporarily interrupt the Mac's network connection, then restore it.
3. Wait for the app to detect transport loss. A very brief interruption may leave SSH connected and not exercise reconnect.
4. When the saved entry offers **Connect**, reconnect and confirm recovery.

Warm reconnect retains process-local continuity. Keep the backend running throughout this test.

### Backend restart and replacement

1. Stop the Ubuntu backend with **Ctrl+C**.
2. Restart it with the same socket command.
3. Once the Mac entry is disconnected, reconnect through it.
4. Confirm that the app communicates session replacement and updates the saved session ID.

The backend mints a new session on restart; replacement should not be presented as seamless continuation.

### Remove a saved entry

Click **Remove**. This forgets local bookkeeping and does not terminate the remote backend. Confirm that the backend terminal continues running.

These are additional manual checks, not recorded results from the setup session. The process monitor uses Standard Widgets and does not test Terminal-extension continuity.

## 9. Troubleshooting

| Symptom | Check or remedy |
| --- | --- |
| `subsystem request failed` | Inspect `sshd -T`, validate and reload SSH on the host/port you are actually connecting to. |
| Socket missing or connection refused from the bridge | Start the monitor and ensure both commands use `/home/alex/.srui/run/monitor.sock`. |
| Socket ownership, mode, or peer validation error | Run the backend and SSH login as `alex`, without sudo, using the private socket directory. |
| Interactive SSH works but the app fails | Run the BatchMode/StrictHostKeyChecking probe; configure key/agent access and known hosts for the same endpoint. |
| Unknown or changed host key | Verify host identity through a trusted channel and correct the known-hosts entry as appropriate. Do not disable verification. |
| SSH connection refused or timed out | Check `systemctl status ssh`, hostname/port, and host/network firewall rules. |
| A second backend refuses to bind | Keep the existing process or stop it deliberately before restarting. Do not remove a live socket to force another instance. |
| Connected entry shows **Open** | Use **Open** to bring its existing session window forward. |

### Stale executable crash encountered during this setup

An existing executable in the old checkout entered the canned demo and crashed with:

```text
RendererAppKit/RendererDemo.swift:40: Fatal error: Initial renderer demo transaction failed
```

The backtrace included `RendererDemo.run()`. This happened before any remote connection. That binary did not match the current source's no-argument connection-manager entry point.

Building the current source in a fresh worktree and launching the resulting executable resolved the issue, with no product source changes. Do not assume an existing binary is current merely because it exists, or launch a similarly named binary from another checkout. Follow step 7.

The repository also documents an editing-tool mtime hazard in [AGENTS.md](AGENTS.md) that can cause incremental builds to reuse stale code. A fresh worktree avoids those old build artifacts.

## References and observed outcome

- [Process-monitor operational guide](examples/process-monitor/README.md)
- [Task 38 requirements](SRUI_Implementation_Plan.md)
- [Build and architecture guidance](CLAUDE.md)

During this setup, the user confirmed that the remote SSH subsystem probe succeeded. After rebuilding and launching the current Mac client, the user confirmed that the UI worked. The reconnect, replacement, and process-action checks above remain steps for the reader to perform, rather than claimed results from that session.
