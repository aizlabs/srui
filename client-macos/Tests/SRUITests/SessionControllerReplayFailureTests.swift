//
// SessionControllerReplayFailureTests.swift
// SRUITests
//
// Controller-owned failure lifecycle for background pending-event replay retries (§18, §18.2, §4 inv. 13).
//

import Testing
import Foundation
import SemanticModel
import Protocol
@testable import Session
import TransportSSH

/// Lock-guarded counter readable from the nonisolated `onFailure` trampoline.
private final class ReplayTestCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    func increment() {
        lock.lock()
        defer { lock.unlock() }
        count += 1
    }

    func store(_ newValue: Int) {
        lock.lock()
        defer { lock.unlock() }
        count = newValue
    }
}

/// Transport whose per-send outcome is scripted by send index, so the resume write, the initial
/// replay write, and the background retry write are separately controllable without sleeps.
///
/// Any send past the script fails and is still recorded, so a retry that outlives teardown shows up
/// as an extra frame instead of disappearing.
private actor ScriptedReplayTransport: Transport {
    enum SendVerdict: Sendable {
        case succeed
        case fail(String)
    }

    private let script: [SendVerdict]
    private var sentFrames: [Data] = []
    private let stream: AsyncThrowingStream<Data, Error>
    private let streamContinuation: AsyncThrowingStream<Data, Error>.Continuation
    /// Nonisolated so the controller's `onFailure` callback can sample it synchronously.
    nonisolated let closeCalls = ReplayTestCounter()

    init(script: [SendVerdict]) {
        self.script = script
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.streamContinuation = continuation
    }

    var sentFrameCount: Int { sentFrames.count }

    func frame(at index: Int) -> Data? {
        sentFrames.indices.contains(index) ? sentFrames[index] : nil
    }

    /// Pushes a server frame into the controller's real receive loop.
    func deliver(message: SRUIMessage) throws {
        streamContinuation.yield(try SRUIFraming.encodeFramed(message))
    }

    /// Resumes once the transport has been closed, failing the test instead of hanging if the
    /// controller never takes ownership of the teardown.
    nonisolated func waitForFirstClose(timeout: Duration = .seconds(2)) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while closeCalls.value == 0 {
            guard clock.now < deadline else {
                throw AsyncTestTimeout(description: "transport close after a terminal failure")
            }
            await Task.yield()
        }
    }

    func send(data: Data) async throws {
        let index = sentFrames.count
        sentFrames.append(data)
        let verdict = index < script.count ? script[index] : .fail("unscripted send #\(index)")
        if case .fail(let message) = verdict {
            throw TransportError.ioError(message)
        }
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        closeCalls.increment()
        streamContinuation.finish()
    }
}

@Suite("SessionController Replay Failure Tests")
struct SessionControllerReplayFailureTests {

    private func event(in frame: Data) throws -> Event {
        let message = try decodeFramedMessage(from: frame)
        guard case .event(let wire) = message.msg else {
            throw AsyncTestTimeout(description: "frame did not carry a CLIENT EVENT")
        }
        return try ProtocolDecoder().validateAndConvertEvent(wire: wire)
    }

    @Test(
        "A failed background replay retry fails the session and the controller owns the teardown",
        .bug("https://github.com/aizlabs/srui/issues/17")
    )
    func backgroundReplayRetryFailureTearsDownThroughController() async throws {
        // Seed one unacknowledged event on the connection that later "dies".
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox(
            replayRetryInitialDelay: .zero,
            replayRetryMaximumDelay: .zero
        )
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: seedClient
        )
        await seedClient.close()
        await seedServer.close()

        // Send #0 is CLIENT RESUME, send #1 is the initial pending-event replay, send #2 is the
        // first background retry and is the failure under test.
        let transport = ScriptedReplayTransport(script: [
            .succeed,
            .succeed,
            .fail("simulated background replay failure"),
        ])
        let controller = SessionController(
            transport: transport,
            outbox: outbox,
            sessionId: "session-a"
        )

        let (failures, failureContinuation) = AsyncStream<SessionFailure>.makeStream()
        let failureCount = ReplayTestCounter()
        let closeCallsWhenReported = ReplayTestCounter()
        controller.onFailure = { failure in
            failureCount.increment()
            // Sampled before the controller's own `transport.close()`: proves the outbox did not
            // close a transport it does not own.
            closeCallsWhenReported.store(transport.closeCalls.value)
            failureContinuation.yield(failure)
        }

        try await controller.start()
        let resumeFrame = try #require(await transport.frame(at: 0))
        guard case .clientResume(let resume) = try decodeFramedMessage(from: resumeFrame).msg else {
            Issue.record("First frame was not a CLIENT RESUME")
            return
        }
        #expect(resume.sessionID == "session-a")
        #expect(resume.lastAckedEventSeq == 0)

        // The server accepts the resume with the event still unsettled, so the outbox replays it
        // once and then keeps retrying until an acknowledgement arrives (§18.2).
        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-a"
        resumeOk.lastProcessedEventSeq = 0
        var response = SRUIMessage()
        response.serverResumeOk = resumeOk
        try await transport.deliver(message: response)

        var failureIterator = failures.makeAsyncIterator()
        let failure = try #require(await failureIterator.next())
        guard case .transportEnded(let message) = failure else {
            Issue.record("Expected .transportEnded, got \(failure)")
            return
        }
        #expect(message.contains("pending event replay retry failed"))
        #expect(message.contains("simulated background replay failure"))
        #expect(closeCallsWhenReported.value == 0)

        // The controller closes the transport itself, exactly once, after reporting.
        try await transport.waitForFirstClose()
        #expect(transport.closeCalls.value == 1)
        #expect(failureCount.value == 1)

        // The retry loop is stopped and the data plane stays shut for this session.
        #expect(await outbox.isRetryingPendingEvents == false)
        #expect(controller.isEventDispatchEnabled == false)
        #expect(controller.isDiverged)
        await #expect(throws: SessionDispatchError.resumeNotConfirmed) {
            try await controller.sendActivate(nodeId: NodeId(8))
        }

        // The initial replay and the failed retry both carried the original event identity, and the
        // intent is still retained for the next resume (§18.2).
        #expect(await transport.sentFrameCount == 3)
        let initialReplay = try event(in: try #require(await transport.frame(at: 1)))
        let retry = try event(in: try #require(await transport.frame(at: 2)))
        #expect(initialReplay.eventId == pending.eventId)
        #expect(initialReplay.eventSeq == pending.eventSeq)
        #expect(retry.eventId == pending.eventId)
        #expect(retry.eventSeq == pending.eventSeq)
        #expect(await outbox.pendingCount == 1)

        // A surviving retry loop would write again immediately (the retry delay is zero), so no
        // further frame may appear once the scheduler has drained.
        for _ in 0..<64 {
            await Task.yield()
        }
        #expect(await transport.sentFrameCount == 3)

        await controller.stop()
        #expect(failureCount.value == 1)
        #expect(await outbox.isRetryingPendingEvents == false)
        failureContinuation.finish()
        await transport.close()
    }
}
