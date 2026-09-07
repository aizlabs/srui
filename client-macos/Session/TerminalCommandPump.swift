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
    private enum Item {
        case input(NodeId, Data)
        case resize(NodeId, UInt32, UInt32, UInt32, UInt32)
    }

    private var queue: [Item] = []
    private var drainTask: Task<Void, Never>?
    private var latestSize: [NodeId: (UInt32, UInt32, UInt32, UInt32)] = [:]
    private var connected = false
    private var transport: (any Transport)?

    func attach(transport: any Transport) {
        self.transport = transport
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
        queue.removeAll { item in
            if case .input = item { return true }
            return false
        }
    }

    func enqueueInput(streamID: NodeId, data: Data) {
        guard connected, !data.isEmpty, data.count <= maxTerminalInputBytes else { return }
        queue.append(.input(streamID, data))
        kick()
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
            guard let transport else { continue }
            do {
                switch item {
                case .input(let id, let data):
                    var input = SRUITerminalInput()
                    input.streamID = id.value
                    input.data = data
                    var envelope = SRUIMessage()
                    envelope.terminalInput = input
                    try await transport.send(
                        data: SRUIFraming.encodeFramed(envelope),
                        logicalClass: .terminalHigh
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
                    try await transport.send(
                        data: SRUIFraming.encodeFramed(envelope),
                        logicalClass: .terminalHigh
                    )
                }
            } catch {
                SessionDiagnostics.error("Terminal command send failed: \(error)")
            }
        }
    }
}
