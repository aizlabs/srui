//
// LogicalChannelScheduler.swift
// LogicalChannelScheduling
//
// Weighted logical-channel selector shared with the Rust sessiond writer (§18.2, §19.2).
//
// This module is dependency-free Swift: no Foundation, Darwin, AppKit, Security,
// Network, or other Apple-platform imports. Linux CI builds and tests it on
// Ubuntu; TransportSSH imports it for production drain on macOS.
//
// Logical classification is independent of protobuf/Core semantics: SSH and TCP
// serialize selected frames onto one byte stream, while a future QUIC binding
// may map the same classes to independent streams without changing Core
// messages. This module does not implement QUIC.
//

/// Logical traffic class used by the outbound scheduler (§19.2).
public enum LogicalChannelClass: Int, CaseIterable, Sendable, Hashable {
    /// Handshake, resume, `SERVER EVENT_ACK`, and future wire-error envelopes.
    case control
    /// Semantic user events.
    case input
    /// Committed UI transactions.
    case ui
    /// Interactive PTY bytes that should preempt bulk terminal output.
    case terminalHigh
    /// Bulk terminal output.
    case terminalNormal
    /// Resource metadata and chunks (images, attachments).
    case resource

}

/// Weighted round-robin selector over the generated shared service cycle.
public struct LogicalChannelScheduler: Sendable {

    private var cursor = 0

    public init() {}

    /// Selects the next ready class, skipping empty lanes without consuming a write.
    ///
    /// The cursor advances past every inspected slot, including skipped empty lanes, so
    /// fairness continues across calls. Returns `nil` when no lane is ready.
    public mutating func selectNext(ready: (LogicalChannelClass) -> Bool) -> LogicalChannelClass? {
        for _ in 0..<Self.serviceCycle.count {
            let logicalClass = Self.serviceCycle[cursor]
            cursor = (cursor + 1) % Self.serviceCycle.count
            if ready(logicalClass) {
                return logicalClass
            }
        }
        return nil
    }
}
