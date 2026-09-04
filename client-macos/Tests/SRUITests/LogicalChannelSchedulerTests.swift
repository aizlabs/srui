//
// LogicalChannelSchedulerTests.swift
// SRUITests
//
// Deterministic saturation, fairness, and call-site classification tests (§18.2, §19.2).
//

import Testing
import Foundation
import SemanticModel
import Protocol
@testable import Session
@testable import TransportSSH

private let resourceTokenSize = 16 * 1024

private func resourceToken(_ id: UInt32) -> Data {
    var data = Data(count: resourceTokenSize)
    data[0] = 0xAB
    var bigEndian = id.bigEndian
    withUnsafeBytes(of: &bigEndian) { bytes in
        data.replaceSubrange(1..<5, with: bytes)
    }
    return data
}

private func tokenId(_ data: Data) -> UInt32? {
    guard data.count == resourceTokenSize, data[0] == 0xAB else { return nil }
    return data.subdata(in: 1..<5).withUnsafeBytes { $0.load(as: UInt32.self).bigEndian }
}

private final class GatedByteSink: @unchecked Sendable {
    private let condition = NSCondition()
    private var permits = 0
    private var entered = 0
    private var failure: (any Error)?
    private(set) var dispatched: [Data] = []

    func write(_ data: Data) throws {
        condition.lock()
        entered += 1
        condition.broadcast()
        while permits == 0 && failure == nil {
            condition.wait()
        }
        if let failure {
            condition.unlock()
            throw failure
        }
        permits -= 1
        dispatched.append(data)
        condition.broadcast()
        condition.unlock()
    }

    func waitUntilDispatched(_ count: Int) {
        condition.lock()
        while dispatched.count < count && failure == nil {
            condition.wait()
        }
        condition.unlock()
    }

    func snapshot() -> [Data] {
        condition.lock()
        defer { condition.unlock() }
        return dispatched
    }

    func waitUntilEntered(_ count: Int) {
        condition.lock()
        while entered < count {
            condition.wait()
        }
        condition.unlock()
    }

    func release(_ count: Int = 1) {
        condition.lock()
        permits += count
        condition.broadcast()
        condition.unlock()
    }

    func fail(_ error: any Error) {
        condition.lock()
        failure = error
        condition.broadcast()
        condition.unlock()
    }
}

private actor RecordingTransport: Transport {
    struct RecordedWrite: Sendable {
        let data: Data
        let logicalClass: LogicalChannelClass
    }

    private(set) var writes: [RecordedWrite] = []
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation
    }

    func send(data: Data) async throws {
        try await send(data: data, logicalClass: .control)
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        writes.append(RecordedWrite(data: data, logicalClass: logicalClass))
    }

    func recordedWrites() -> [RecordedWrite] { writes }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> { stream }

    func close() async {
        continuation.finish()
    }
}

@Suite("Logical Channel Scheduler (§19.2)")
struct LogicalChannelSchedulerTests {

    @Test("Service cycle matches the documented 24-slot sequence")
    func serviceCycleMatchesDocumentedSequence() {
        #expect(LogicalChannelScheduler.serviceCycle == [
            .control, .input, .ui,
            .control, .input, .terminalHigh,
            .control, .input, .ui,
            .control, .input, .terminalNormal,
            .control, .input, .ui,
            .control, .input, .terminalHigh,
            .ui, .control, .input,
            .terminalNormal, .terminalHigh, .resource,
        ])
        #expect(LogicalChannelScheduler.serviceCycle.count == 24)
    }

    @Test("Saturated cycle preserves FIFO and documented service gaps")
    func saturatedCyclePreservesFIFOAndBounds() throws {
        var queues: [LogicalChannelClass: [UInt32]] = Dictionary(
            uniqueKeysWithValues: LogicalChannelClass.allCases.map { ($0, []) }
        )
        for logicalClass in LogicalChannelClass.allCases {
            queues[logicalClass] = Array(0..<12)
        }

        var scheduler = LogicalChannelScheduler()
        var lastIndex: [LogicalChannelClass: Int] = [:]
        var nextExpected: [LogicalChannelClass: UInt32] = Dictionary(
            uniqueKeysWithValues: LogicalChannelClass.allCases.map { ($0, 0) }
        )

        for index in 0..<(LogicalChannelScheduler.serviceCycle.count * 6) {
            for logicalClass in LogicalChannelClass.allCases {
                if queues[logicalClass]?.isEmpty == true {
                    queues[logicalClass, default: []].append(nextExpected[logicalClass] ?? 0)
                }
            }
            let selected = scheduler.selectNext { candidate in
                !(queues[candidate] ?? []).isEmpty
            }
            let logicalClass = try #require(selected)
            let token = queues[logicalClass]!.removeFirst()
            #expect(token == nextExpected[logicalClass])
            nextExpected[logicalClass, default: 0] += 1
            if let previous = lastIndex[logicalClass] {
                #expect(index - previous <= logicalClass.maxServiceGap)
            }
            lastIndex[logicalClass] = index
        }
    }

    @Test("Empty lanes are skipped without consuming a write")
    func emptyLanesAreSkipped() {
        var resource = [UInt32]([1, 2])
        var scheduler = LogicalChannelScheduler()
        let first = scheduler.selectNext { $0 == .resource && !resource.isEmpty }
        #expect(first == .resource)
        #expect(resource.removeFirst() == 1)
        let second = scheduler.selectNext { $0 == .resource && !resource.isEmpty }
        #expect(second == .resource)
        #expect(resource.removeFirst() == 2)
        #expect(scheduler.selectNext { $0 == .resource && !resource.isEmpty } == nil)
    }

    @Test("Two control tokens remain separate and FIFO")
    func twoControlTokensRemainSeparateAndFIFO() {
        var control = [UInt32]([1, 2])
        var scheduler = LogicalChannelScheduler()
        #expect(scheduler.selectNext { $0 == .control && !control.isEmpty } == .control)
        #expect(control.removeFirst() == 1)
        #expect(scheduler.selectNext { $0 == .control && !control.isEmpty } == .control)
        #expect(control.removeFirst() == 2)
    }

    @Test("Resource backlog yields to newly ready control, input, and UI")
    func resourceBacklogYieldsToHigherClasses() async throws {
        let sink = GatedByteSink()
        let writer = SocketWriter(label: "test.scheduler-gate", testSink: { try sink.write($0) })
        let latch = SocketReadLatch()

        try await withThrowingTaskGroup(of: Void.self) { group in
            defer {
                writer.stop()
                sink.fail(TransportError.closed)
                latch.stop()
            }

            group.addTask {
                _ = try? await writer.write(
                    resourceToken(0),
                    logicalClass: .resource,
                    claiming: latch
                )
            }
            sink.waitUntilEntered(1)

            for id in UInt32(1)..<UInt32(200) {
                let payload = resourceToken(id)
                group.addTask {
                    _ = try? await writer.write(
                        payload,
                        logicalClass: .resource,
                        claiming: latch
                    )
                }
            }
            try await waitUntilQueued(199, in: .resource, writer: writer)

            let control = Data("control-probe".utf8)
            let input = Data("input-probe".utf8)
            let ui = Data("ui-probe".utf8)

            group.addTask {
                _ = try? await writer.write(control, logicalClass: .control, claiming: latch)
            }
            try await waitUntilQueued(1, in: .control, writer: writer)

            group.addTask {
                _ = try? await writer.write(input, logicalClass: .input, claiming: latch)
            }
            try await waitUntilQueued(1, in: .input, writer: writer)

            group.addTask {
                _ = try? await writer.write(ui, logicalClass: .ui, claiming: latch)
            }
            try await waitUntilQueued(1, in: .ui, writer: writer)

            sink.release(8)
            sink.waitUntilDispatched(4)

            let dispatched = sink.snapshot()
            #expect(dispatched.count >= 4)
            #expect(tokenId(dispatched[0]) == 0)

            let rest = dispatched.dropFirst()
            let controlAt = try #require(rest.firstIndex(of: control))
            let inputAt = try #require(rest.firstIndex(of: input))
            let uiAt = try #require(rest.firstIndex(of: ui))
            let probeEnd = try #require([controlAt, inputAt, uiAt].max())
            let resourceBeforeProbes = rest.prefix(through: probeEnd).filter {
                tokenId($0) != nil
            }.count
            #expect(
                resourceBeforeProbes == 0,
                "no second resource token may precede control/input/UI probes"
            )
        }
    }

    private func waitUntilQueued(
        _ expectedCount: Int,
        in logicalClass: LogicalChannelClass,
        writer: SocketWriter
    ) async throws {
        var spins = 0
        while writer.queuedCount(for: logicalClass) < expectedCount {
            spins += 1
            try #require(spins < 100_000, "writes never entered the expected scheduler queue")
            await Task.yield()
        }
    }

    @Test("SessionController sends HELLO and RESUME as control")
    func sessionControllerClassifiesHandshakeAsControl() async throws {
        let helloTransport = RecordingTransport()
        let helloController = SessionController(transport: helloTransport)
        try await helloController.start()
        let helloWrites = await helloTransport.recordedWrites()
        await helloController.stop()
        await helloTransport.close()

        #expect(helloWrites.count == 1)
        #expect(helloWrites[0].logicalClass == .control)
        let helloMessage = try decodeFramedMessage(from: helloWrites[0].data)
        guard case .clientHello = helloMessage.msg else {
            Issue.record("expected CLIENT HELLO, got \(String(describing: helloMessage.msg))")
            return
        }

        let resumeTransport = RecordingTransport()
        let resumeController = SessionController(transport: resumeTransport, sessionId: "session-resume")
        try await resumeController.start()
        let resumeWrites = await resumeTransport.recordedWrites()
        await resumeController.stop()
        await resumeTransport.close()

        #expect(resumeWrites.count == 1)
        #expect(resumeWrites[0].logicalClass == .control)
        let resumeMessage = try decodeFramedMessage(from: resumeWrites[0].data)
        guard case .clientResume = resumeMessage.msg else {
            Issue.record("expected CLIENT RESUME, got \(String(describing: resumeMessage.msg))")
            return
        }
    }

    @Test("EventOutbox sends original and replayed events as input")
    func eventOutboxClassifiesEventsAsInput() async throws {
        let transport = RecordingTransport()
        let outbox = EventOutbox()
        _ = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            via: transport
        )
        _ = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(1),
            via: transport
        )
        try await outbox.resendPendingEvents(via: transport)

        let writes = await transport.recordedWrites()
        #expect(writes.count == 4)
        for write in writes {
            #expect(write.logicalClass == .input)
            let message = try decodeFramedMessage(from: write.data)
            guard case .event = message.msg else {
                Issue.record("expected semantic event, got \(String(describing: message.msg))")
                return
            }
        }
        await transport.close()
    }
}
