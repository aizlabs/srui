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

    /// Classifies OpenSSH's own diagnostics (`sshconnect.c`), or returns `nil` when the failure
    /// was not a host-key failure. The most specific message wins: OpenSSH ends every one of these
    /// cases with "Host key verification failed."
    init?(sshMessage message: String) {
        if message.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
            || message.contains("has changed and you have requested strict checking")
            || message.contains("POSSIBLE DNS SPOOFING DETECTED") {
            self = .changed
        } else if message.contains("REVOKED HOST KEY DETECTED") {
            self = .revoked
        } else if message.contains("host key is known for")
            && message.contains("you have requested strict checking") {
            self = .unknown
        } else if message.contains("Host key verification failed") {
            self = .unverified
        } else {
            return nil
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
