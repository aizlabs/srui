# SRUI macOS SSH Transport Binding (`TransportSSH`)

This module provides the secure transport binding for the macOS SRUI client as specified in **§19 (SSH transport binding)** and **§19.1 (recommended SSH posture)** of `SRUI_Semantic_Remote_UI_Design_v0.6.md`.

---

## Architectural Choices & Rationale (§19.1, §25)

The SRUI reference macOS client executes the system OpenSSH binary (`/usr/bin/ssh`) directly via `Foundation.Process` rather than embedding a third-party C library (e.g. `libssh2`/`libssh`).

### Key Benefits:
1. **Spec Alignment (§19.1):** *"The client should rely on the user's existing `ssh`/agent/keychain infrastructure rather than implementing private-key handling itself."*
2. **Keychain & Agent Integration:** Transparently inherits macOS Keychain, `ssh-agent`, hardware security keys (FIDO2/U2F), PKCS#11 smartcards, `~/.ssh/config` host definitions, and host certificate validation without requiring private key management in the client.
3. **Zero Shell Interpolation:** Arguments are passed directly as an array of `[String]` to the OS exec vector; no shell (`/bin/sh`) is invoked.
4. **Stream & Diagnostic Isolation:** The binary protocol stream flows over `stdin`/`stdout`, while `stderr` is captured asynchronously and redirected to diagnostics.

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
| **Fixed Subsystem Request** | `-s srui` | Direct subsystem request avoiding shell command execution |
| **Identity Isolation** | `-o IdentitiesOnly=yes` (when `-i` is set) | Restricts authentication to the specified key |

### `extraOptions` Escape Hatch
`SSHConfiguration` provides an `extraOptions: [String: String]` dictionary for advanced users who require customized OpenSSH `-o` options (e.g. custom proxy commands, ciphers, or keepalive parameters), while keeping strict §19.1 defaults.

---

## Running Live Over SSH

### 1. Server-Side OpenSSH Subsystem Setup
Add to `/etc/ssh/sshd_config` or `/etc/ssh/sshd_config.d/srui.conf`:
```text
Subsystem srui /usr/local/bin/srui-ssh-bridge
```

### 2. Client-Side Launch
Launch `RendererDemoApp` over SSH:
```bash
swift run --package-path client-macos RendererDemoApp --ssh remote.host.example.com --user username
```

With custom port or identity:
```bash
swift run --package-path client-macos RendererDemoApp --ssh 127.0.0.1 --port 2222 --identity ~/.ssh/id_ed25519
```
