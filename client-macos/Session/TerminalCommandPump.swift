//
// TerminalCommandPump.swift
// Session
//
// Connection-scoped PTY input/resize writer (§19.2, §21).
// One pump preserves keystroke order and coalesces resize; it does not spawn a
// Task per keystroke and does not retain keys while disconnected.
//

import Foundation
import Protocol
import SemanticModel
import Terminal
import TransportSSH

actor TerminalCommandPump {
    typealias SendIfAuthorized = @Sendable (
        Data,
        LogicalChannelClass
    ) async throws -> Bool

    private enum Item {
        case input(NodeId, Data)
        case resize(NodeId, UInt32, UInt32, UInt32, UInt32)
    }

    private var queue: [Item] = []
    private var drainTask: Task<Void, Never>?
    private var latestSize: [NodeId: (UInt32, UInt32, UInt32, UInt32)] = [:]
    private var connected = false
    private var sendIfAuthorized: SendIfAuthorized?

    func attach(transport: any Transport) {
        attach { data, logicalClass in
            try await transport.send(data: data, logicalClass: logicalClass)
            return true
        }
    }

    func attach(sendIfAuthorized: @escaping SendIfAuthorized) {
        self.sendIfAuthorized = sendIfAuthorized
        connected = true
        for (id, size) in latestSize {
            queue.removeAll { item in
                if case .resize(let other, _, _, _, _) = item { return other == id }
                return false
            }
            queue.append(.resize(id, size.0, size.1, size.2, size.3))
        }
        kick()
    }

    func disconnect() {
        connected = false
        sendIfAuthorized = nil
        queue.removeAll { item in
            if case .input = item { return true }
            return false
        }
    }

    func resetForReplacementSession(
        sendIfAuthorized: @escaping SendIfAuthorized
    ) {
        queue.removeAll(keepingCapacity: true)
        latestSize.removeAll(keepingCapacity: true)
        self.sendIfAuthorized = sendIfAuthorized
    }

    var retainedResizeCountForTesting: Int {
        latestSize.count
    }

    func waitUntilIdleForTesting() async {
        await drainTask?.value
    }

    func prune(retainedStreamIDs: Set<NodeId>) {
        latestSize = latestSize.filter { retainedStreamIDs.contains($0.key) }
        queue.removeAll { item in
            switch item {
            case .input(let id, _), .resize(let id, _, _, _, _):
                return !retainedStreamIDs.contains(id)
            }
        }
    }

    func removeStream(streamID: NodeId) {
        latestSize.removeValue(forKey: streamID)
        queue.removeAll { item in
            switch item {
            case .input(let id, _), .resize(let id, _, _, _, _):
                return id == streamID
            }
        }
    }

    func enqueueInput(streamID: NodeId, data: Data) {
        guard connected, !data.isEmpty else { return }
        // A clipboard paste routinely exceeds the per-envelope limit. Dropping it would send no
        // input at all, so split it into ordered TerminalInput frames instead: the PTY is a byte
        // stream, chunks reassemble exactly, and the single bracketed-paste begin/end pair the
        // encoder produced is preserved because only the encoded byte run is cut (§21, §26).
        var remaining = data[...]
        while !remaining.isEmpty {
            let take = Self.chunkLength(of: remaining, limit: maxTerminalInputBytes)
            let cut = remaining.index(remaining.startIndex, offsetBy: take)
            queue.append(.input(streamID, Data(remaining[..<cut])))
            remaining = remaining[cut...]
        }
        kick()
    }

    /// Longest prefix of at most `limit` bytes that does not cut a UTF-8 scalar in half, so a
    /// remote line editor never reads a truncated codepoint. Falls back to `limit` for byte runs
    /// that are not valid UTF-8 at that boundary, so every chunk makes progress.
    private static func chunkLength(of bytes: Data.SubSequence, limit: Int) -> Int {
        guard limit < bytes.count else { return bytes.count }
        var take = limit
        let floor = max(1, limit - 3)
        while take > floor {
            let byte = bytes[bytes.index(bytes.startIndex, offsetBy: take)]
            if byte & 0xC0 != 0x80 { return take }
            take -= 1
        }
        return limit
    }

    func enqueueResize(streamID: NodeId, columns: UInt32, rows: UInt32, pixelWidth: UInt32, pixelHeight: UInt32) {
        latestSize[streamID] = (columns, rows, pixelWidth, pixelHeight)
        guard connected else { return }
        queue.removeAll { item in
            if case .resize(let other, _, _, _, _) = item { return other == streamID }
            return false
        }
        queue.append(.resize(streamID, columns, rows, pixelWidth, pixelHeight))
        kick()
    }

    private func kick() {
        guard drainTask == nil else { return }
        drainTask = Task { await self.drain() }
    }

    private func drain() async {
        defer { drainTask = nil }
        while connected, !queue.isEmpty {
            let item = queue.removeFirst()
            guard let sendIfAuthorized else { continue }
            do {
                switch item {
                case .input(let id, let data):
                    var input = SRUITerminalInput()
                    input.streamID = id.value
                    input.data = data
                    var envelope = SRUIMessage()
                    envelope.terminalInput = input
                    _ = try await sendIfAuthorized(
                        SRUIFraming.encodeFramed(envelope),
                        .terminalHigh
                    )
                case .resize(let id, let cols, let rows, let width, let height):
                    var resize = SRUITerminalResize()
                    resize.streamID = id.value
                    resize.columns = cols
                    resize.rows = rows
                    resize.pixelWidth = width
                    resize.pixelHeight = height
                    var envelope = SRUIMessage()
                    envelope.terminalResize = resize
                    _ = try await sendIfAuthorized(
                        SRUIFraming.encodeFramed(envelope),
                        .terminalHigh
                    )
                }
            } catch {
                SessionDiagnostics.error("Terminal command send failed: \(error)")
            }
        }
    }
}
