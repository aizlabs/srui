//
// HostKeyRejection.swift
// ConnectionManager
//
// Understandable diagnostics for a host key OpenSSH refused (§19.1, §25; PX-023).
//
// The trust decision stays OpenSSH's: every attempt runs with `StrictHostKeyChecking=yes` and
// `BatchMode=yes` against the user's own known_hosts, so an unknown, changed or revoked key ends
// the attempt before any SRUI byte flows. This only reads the failure `ssh` printed and says, in
// the blocking alert, which case it was and how to verify the key outside SRUI. Nothing here adds,
// replaces or accepts a key, and the alert offers no way to connect anyway.
//

import Foundation

enum HostKeyRejection: Equatable, Sendable {
    /// known_hosts has no key for this host.
    case unknown
    /// known_hosts holds a different key for this host.
    case changed
    /// The offered key is marked revoked.
    case revoked
    /// OpenSSH reported a host-key verification failure without saying which kind.
    case unverified

    /// The line OpenSSH's `fatal()` ends every host-key refusal with (`sshconnect2.c`).
    static let verdictLine = "Host key verification failed."

    /// Classifies OpenSSH's own diagnostics (`sshconnect.c`), or returns `nil` when the failure
    /// was not a host-key failure.
    ///
    /// Structural, not a phrase search (PX-023 round 3). `ssh` runs with `LogLevel=ERROR`, so its
    /// stderr holds no server `Banner`; but a server's `SSH_MSG_DISCONNECT` reason is still logged
    /// at ERROR ("Received disconnect from …: <text>"), and an authenticated subsystem's own stderr
    /// shares the stream. So the verdict must be the **last** line, which only `ssh`'s fatal exit
    /// writes (a disconnect is followed by "Disconnected from …", an authentication failure by
    /// "Permission denied …"), and the kind is read from the line OpenSSH writes just before it.
    init?(sshMessage message: String) {
        let lines = message
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { $0.isEmpty == false }
        guard lines.last == Self.verdictLine else { return nil }
        let reason = lines.count >= 2 ? lines[lines.count - 2] : ""
        if reason.hasPrefix("No "),
           reason.contains(" host key is known for "),
           reason.hasSuffix(" and you have requested strict checking.") {
            self = .unknown
        } else if reason.hasPrefix("Host key for "),
                  reason.hasSuffix(" has changed and you have requested strict checking.") {
            self = .changed
        } else if reason == "impersonate this host.",
                  lines.contains(where: { $0.contains("WARNING: REVOKED HOST KEY DETECTED!") }) {
            self = .revoked
        } else {
            self = .unverified
        }
    }

    func alertMessage(for connection: SavedConnection, sshMessage: String) -> String {
        let endpoint = ConnectionEndpoint.displayString(
            host: connection.host,
            port: connection.port,
            user: connection.user
        )
        // The name OpenSSH files the key under: `[host]:port` for any port but 22.
        let knownHostsName: String
        if let port = connection.port, port != 22 {
            knownHostsName = "[\(connection.host)]:\(port)"
        } else {
            knownHostsName = connection.host
        }
        let portArgument = connection.port.map { "-p \($0) " } ?? ""
        let terminalCommand = "ssh \(portArgument)\(connection.user)@\(connection.host)"

        let explanation: String
        switch self {
        case .unknown:
            explanation = """
            SRUI did not connect to \(endpoint). Your known_hosts file has no host key for \(knownHostsName), \
            so OpenSSH cannot verify that this is the server you mean. SRUI never adds a host key by itself. \
            Check the key’s fingerprint with the server’s administrator, record it by connecting once from \
            Terminal (\(terminalCommand)), then try again.
            """
        case .changed:
            explanation = """
            SRUI did not connect to \(endpoint). The server’s host key does not match the key your known_hosts \
            file holds for \(knownHostsName). The server may have been reinstalled, or someone may be \
            intercepting the connection. SRUI never replaces a known host key. Check the new key’s fingerprint \
            with the server’s administrator before you remove the old entry (ssh-keygen -R '\(knownHostsName)') \
            and record the new one from Terminal (\(terminalCommand)).
            """
        case .revoked:
            explanation = """
            SRUI did not connect to \(endpoint). The server offered a host key that your OpenSSH configuration \
            marks as revoked. Contact the server’s administrator.
            """
        case .unverified:
            explanation = """
            SRUI did not connect to \(endpoint). OpenSSH could not verify the server’s host key against your \
            known_hosts file. Check the entry for \(knownHostsName), then try again.
            """
        }
        return "\(explanation)\n\nOpenSSH reported:\n\(sshMessage)"
    }
}
