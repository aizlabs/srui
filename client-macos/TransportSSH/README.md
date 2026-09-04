# SRUI macOS SSH Transport Binding (`TransportSSH`)

This module provides the secure transport binding for the macOS SRUI client as specified in **§19 (SSH transport binding)** and **§19.1 (recommended SSH posture)** of `SRUI_Semantic_Remote_UI_Design_v0.6.md`.

---

## Logical channel scheduler (§19.2)

Outbound frames are classified independently of protobuf/Core semantics. SSH and TCP serialize
whichever frame the scheduler selects onto **one** byte stream. A future QUIC binding may map the
same classes to independent streams without changing Core messages. This module does not implement
QUIC.

| Logical class | Priority | Examples |
| :--- | ---: | :--- |
| `control` | highest | `CLIENT HELLO`, `CLIENT RESUME`, `SERVER WELCOME`, resume responses, `SERVER EVENT_ACK` |
| `input` | highest | semantic user events (`EventOutbox`) |
| `ui` | high | committed transactions (reserved on the client outbound path) |
| `terminalHigh` | high | interactive PTY bytes (reserved; Task 30) |
| `terminalNormal` | normal | bulk terminal output (reserved; Task 30) |
| `resource` | low | images and attachments (reserved on the client outbound path) |

`Transport.send(data:)` is the compatibility path and defaults to `control`. Production writers
use `send(data:logicalClass:)`. `SocketWriter` keeps per-class FIFO queues and drains them through
the shared 24-slot weighted cycle:

```text
control, input, ui,
control, input, terminalHigh,
control, input, ui,
control, input, terminalNormal,
control, input, ui,
control, input, terminalHigh,
ui, control, input,
terminalNormal, terminalHigh, resource
```

Empty lanes are skipped without consuming a write. FIFO order is preserved inside each lane, and
the cursor is retained between selections. Maximum head-of-line service distances under continuous
saturation:

| Class | Max dispatched frames |
| :--- | ---: |
| control | 5 |
| input | 5 |
| UI | 8 |
| terminal-high | 12 |
| terminal-normal | 14 |
| resource | 24 |

No scheduler can promise wall-clock delivery when the peer stops reading. The meaningful invariant
is bounded scheduler selections plus at most the currently non-preemptible 16 KiB resource chunk;
the existing 30-second write timeout remains the terminal stalled-peer bound.

---

## Architectural Choices & Rationale (§19.1, §25)

The SRUI reference macOS client executes the system OpenSSH binary (`/usr/bin/ssh`) directly via `Foundation.Process` rather than embedding a third-party C library (e.g. `libssh2`/`libssh`).

### Key Benefits:
1. **Spec Alignment (§19.1):** *"The client should rely on the user's existing `ssh`/agent/keychain infrastructure rather than implementing private-key handling itself."*
2. **Keychain & Agent Integration:** Transparently inherits macOS Keychain, `ssh-agent`, hardware security keys (FIDO2/U2F), PKCS#11 smartcards, `~/.ssh/config` host definitions, and host certificate validation without requiring private key management in the client.
3. **Zero Shell Interpolation:** Arguments are passed directly as an array of `[String]` to the OS exec vector; no shell (`/bin/sh`) is invoked.
4. **Stream & Diagnostic Isolation:** The binary protocol stream flows over `stdin`/`stdout`, while `stderr` is captured on a dedicated reader thread and surfaced only in diagnostics.

Blocking stdout reads run on a dedicated thread (same pattern as `UnixSocketTransport`) so long-lived SSH sessions do not occupy Swift cooperative pool threads (§22.2).

---

## Recommended SSH Posture Checklist (§19.1)

`SSHConfiguration.buildArguments()` automatically applies the normative posture:

| Posture Requirement | Implementation | Purpose |
| :--- | :--- | :--- |
| **No PTY** | `-T` | Disables terminal/pty allocation for clean 8-bit binary transport |
| **No X11 Forwarding** | `-x` | Disables X11 graphical remoting |
| **No Agent Forwarding** | `-a` | Prevents remote server from accessing client ssh-agent keys |
| **No Ad Hoc Port Forwards** | `-o ClearAllForwardings=yes`, `-o ExitOnForwardFailure=yes` | Blocks arbitrary port tunnels configured in `~/.ssh/config` |
| **Fail Closed on Host-Key Changes** | `-o StrictHostKeyChecking=yes` | Refuses connection on host key mismatch or untrusted hosts in batch mode |
| **Fixed Subsystem Request** | `<host> -s srui` | Direct subsystem request avoiding shell command execution |
| **Identity Isolation** | `-o IdentitiesOnly=yes` (when `-i` is set) | Restricts authentication to the specified key |

### `extraOptions` (filtered)

`SSHConfiguration.extraOptions` accepts benign OpenSSH `-o` overrides (for example `Compression=no`). Keys that can weaken §19.1 posture (`StrictHostKeyChecking`, `ClearAllForwardings`, `ForwardAgent`, `*Forward`, `ProxyCommand`, etc.) are **ignored**. Normative posture flags are always appended last so they cannot be overridden.

---

## `SSHTransport` Lifecycle

`SSHTransport` is an `actor` conforming to `Transport`:

1. **Lazy connect:** The first `send(_:)` or `receiveStream()` consumption spawns `/usr/bin/ssh`.
2. **Receive stream:** `receiveStream()` returns an `AsyncThrowingStream<Data, Error>`. Connect failures finish the stream with `TransportError.connectionFailed`.
3. **Stdout reader:** A dedicated thread drains stdout to EOF, then waits for the child process and finishes the stream once (success or `TransportError.connectionFailed` with stderr context).
4. **Shutdown:** `close()` terminates the child; the reader owns the terminal `finish` when a connection was established. Dropping the transport without `close()` still finishes the stream from `deinit`.

---

## Running Live Over SSH

### Prerequisites

- macOS with `/usr/bin/ssh` and `/usr/sbin/sshd` (for integration tests)
- Server: `srui-ssh-bridge` installed and `Subsystem srui` configured in `sshd_config`
- For live integration tests: `cargo build` in `server-rust/` and `examples/counter/`

### 1. Server-Side OpenSSH Subsystem Setup

Add to `/etc/ssh/sshd_config` or `/etc/ssh/sshd_config.d/srui.conf`:

```text
Subsystem srui /usr/local/bin/srui-ssh-bridge
```

### 2. Client-Side Launch

Launch `RendererDemoApp` over SSH (defaults: `BatchMode=yes`, 30s connect timeout):

```bash
swift run --package-path client-macos RendererDemoApp --ssh remote.host.example.com --user username
```

With custom port or identity:

```bash
swift run --package-path client-macos RendererDemoApp --ssh 127.0.0.1 --port 2222 --identity ~/.ssh/id_ed25519
```

Use `--interactive` to allow OpenSSH host-key/passphrase prompts (not recommended for unattended GUI use).

### 3. Verification

```bash
bash scripts/test_task19_counter_ssh.sh
```

Or: `swift test --package-path client-macos --filter SSHTransport`
