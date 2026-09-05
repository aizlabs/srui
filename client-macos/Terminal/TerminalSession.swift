//
// TerminalSession.swift
// Terminal
//
// Per-stream VT state, offset accounting, and snapshot fan-out (§21, §21.2).
// Normative requirement: this target must NEVER import AppKit or Cocoa.
//

import Foundation
import SemanticModel

public enum TerminalResyncCause: Equatable, Sendable {
    case retentionLoss
    case offsetAhead
    case subscriberFallbehind
    case unspecified
    case localGap
}

/// Connection-scoped terminal emulator. One actor owns every stream's grid so remounts
/// and same-session reconnects reuse state; a replaced session clears it.
public actor TerminalSession {
    private struct StreamState {
        var grid: TerminalGrid
        var parser: VTParser
        var nextOffset: UInt64
        var needsRedraw: Bool
        var snapshotWaiters: [UUID: AsyncStream<TerminalSnapshot>.Continuation]
    }

    private var streams: [NodeId: StreamState] = [:]

    public init() {}

    public func resetForReplacementSession() {
        for state in streams.values {
            for waiter in state.snapshotWaiters.values {
                waiter.finish()
            }
        }
        streams.removeAll()
    }

    public func syncPresentNodes(_ present: Set<NodeId>) {
        let stale = streams.keys.filter { !present.contains($0) }
        for id in stale {
            if let state = streams.removeValue(forKey: id) {
                for waiter in state.snapshotWaiters.values {
                    waiter.finish()
                }
            }
        }
    }

    /// Resume map: stream NodeId → next expected byte offset.
    public func streamOffsets() -> [UInt64: UInt64] {
        var map: [UInt64: UInt64] = [:]
        for (id, state) in streams {
            map[id.value] = state.nextOffset
        }
        if map.count > maxTerminalResumeMapEntries {
            return Dictionary(uniqueKeysWithValues: map.sorted { $0.key < $1.key }.prefix(maxTerminalResumeMapEntries))
        }
        return map
    }

    public func snapshot(for streamID: NodeId) -> TerminalSnapshot? {
        guard let state = streams[streamID] else { return nil }
        return state.grid.snapshot(
            streamID: streamID,
            nextOffset: state.nextOffset,
            needsRedraw: state.needsRedraw
        )
    }

    public func snapshots(for streamID: NodeId) -> AsyncStream<TerminalSnapshot> {
        let (stream, continuation) = AsyncStream.makeStream(
            of: TerminalSnapshot.self,
            bufferingPolicy: .bufferingNewest(1)
        )
        var state = streams[streamID] ?? StreamState(
            grid: TerminalGrid(),
            parser: VTParser(),
            nextOffset: 0,
            needsRedraw: false,
            snapshotWaiters: [:]
        )
        if let current = streams[streamID] {
            continuation.yield(
                current.grid.snapshot(
                    streamID: streamID,
                    nextOffset: current.nextOffset,
                    needsRedraw: current.needsRedraw
                )
            )
        }
        let token = UUID()
        state.snapshotWaiters[token] = continuation
        streams[streamID] = state
        continuation.onTermination = { _ in
            Task { await self.dropWaiter(streamID: streamID, token: token) }
        }
        return stream
    }

    public func resize(streamID: NodeId, columns: Int, rows: Int) {
        var state = streams[streamID] ?? StreamState(
            grid: TerminalGrid(columns: columns, rows: rows),
            parser: VTParser(),
            nextOffset: 0,
            needsRedraw: false,
            snapshotWaiters: [:]
        )
        state.grid.resize(columns: columns, rows: rows)
        streams[streamID] = state
        publish(streamID)
    }

    public func applicationCursorKeys(for streamID: NodeId) -> Bool {
        streams[streamID]?.grid.applicationCursorKeys ?? false
    }

    public func bracketedPaste(for streamID: NodeId) -> Bool {
        streams[streamID]?.grid.bracketedPaste ?? false
    }

    /// Applies a `TerminalData` frame using the §21 offset contract.
    @discardableResult
    public func applyData(streamID: NodeId, byteOffset: UInt64, data: Data) throws -> TerminalSnapshot {
        if data.isEmpty { throw TerminalApplyError.emptyFrame }
        if data.count > maxTerminalOutputFrameBytes {
            throw TerminalApplyError.frameTooLarge(data.count)
        }
        let end = try addOffsets(byteOffset, UInt64(data.count))
        var state = streams[streamID] ?? StreamState(
            grid: TerminalGrid(),
            parser: VTParser(),
            nextOffset: 0,
            needsRedraw: false,
            snapshotWaiters: [:]
        )

        if byteOffset == state.nextOffset {
            state.parser.feed(data, into: state.grid)
            state.nextOffset = end
        } else if end <= state.nextOffset {
            // Fully duplicated older frame.
        } else if byteOffset < state.nextOffset {
            let skip = Int(state.nextOffset - byteOffset)
            if skip < data.count {
                state.parser.feed(data.dropFirst(skip), into: state.grid)
                state.nextOffset = end
            }
        } else {
            // Ahead of the local cursor: local resync, keep the session semantic.
            state.grid.reset()
            state.parser.reset()
            state.needsRedraw = true
            state.parser.feed(data, into: state.grid)
            state.nextOffset = end
        }

        streams[streamID] = state
        let snapshot = state.grid.snapshot(
            streamID: streamID,
            nextOffset: state.nextOffset,
            needsRedraw: state.needsRedraw
        )
        publish(streamID)
        return snapshot
    }

    /// Applies `TerminalResyncRequired` without touching semantic session state.
    @discardableResult
    public func applyResync(
        streamID: NodeId,
        requestedOffset: UInt64,
        retainedFromOffset: UInt64,
        resumeAtOffset: UInt64,
        cause: TerminalResyncCause
    ) -> TerminalSnapshot {
        _ = requestedOffset
        _ = retainedFromOffset
        _ = cause
        var state = streams[streamID] ?? StreamState(
            grid: TerminalGrid(),
            parser: VTParser(),
            nextOffset: 0,
            needsRedraw: false,
            snapshotWaiters: [:]
        )
        state.grid.reset()
        state.parser.reset()
        state.nextOffset = resumeAtOffset
        state.needsRedraw = true
        streams[streamID] = state
        let snapshot = state.grid.snapshot(
            streamID: streamID,
            nextOffset: state.nextOffset,
            needsRedraw: true
        )
        publish(streamID)
        return snapshot
    }

    public func acknowledgeRedraw(streamID: NodeId) {
        streams[streamID]?.needsRedraw = false
    }

    private func publish(_ streamID: NodeId) {
        guard let state = streams[streamID] else { return }
        let snapshot = state.grid.snapshot(
            streamID: streamID,
            nextOffset: state.nextOffset,
            needsRedraw: state.needsRedraw
        )
        for waiter in state.snapshotWaiters.values {
            waiter.yield(snapshot)
        }
    }

    private func dropWaiter(streamID: NodeId, token: UUID) {
        guard var state = streams[streamID] else { return }
        state.snapshotWaiters.removeValue(forKey: token)
        streams[streamID] = state
    }

    private func addOffsets(_ a: UInt64, _ b: UInt64) throws -> UInt64 {
        let (result, overflow) = a.addingReportingOverflow(b)
        if overflow { throw TerminalApplyError.offsetOverflow }
        return result
    }
}
