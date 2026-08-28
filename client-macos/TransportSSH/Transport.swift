//
// Transport.swift
// TransportSSH
//
// Transport protocol abstraction for SRUI network and IPC channels (§19, §20.2, §22).
//

import Foundation

/// Errors produced during transport operations.
public enum TransportError: Error, CustomStringConvertible, Sendable {
    case connectionFailed(String)
    case closed
    case ioError(String)
    case timeout

    public var description: String {
        switch self {
        case .connectionFailed(let msg):
            return "Transport connection failed: \(msg)"
        case .closed:
            return "Transport is closed"
        case .ioError(let msg):
            return "Transport I/O error: \(msg)"
        case .timeout:
            return "Transport operation timed out"
        }
    }
}

/// Abstract stream transport interface decoupling the protocol and session layers
/// from the underlying socket or SSH channel implementation (§19, §20.2, §22).
public protocol Transport: Sendable {
    /// Transmits raw framed bytes over the transport connection.
    func send(data: Data) async throws

    /// Returns an asynchronous throwing stream of incoming raw byte chunks from the remote peer.
    func receiveStream() -> AsyncThrowingStream<Data, Error>

    /// Gracefully closes the transport connection and releases underlying resources.
    func close() async
}
