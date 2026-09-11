import AppKit
import CryptoKit
import Darwin
import Foundation
import Protocol
import RendererAppKit
import Resources
import SemanticModel
import Session
import SwiftProtobuf
import Terminal
import TransportSSH
import WebKit

struct CapturedTransportFrame: Sendable {
    let data: Data
    let logicalClass: LogicalChannelClass
}

struct BenchmarkTransportSnapshot: Sendable {
    let outboundAttempts: Int
    let outboundMessages: Int
    let outboundBytes: Int
    let inboundMessages: Int
    let inboundBytes: Int
    let droppedMessages: Int
    let interruptions: Int
    let activeDelayedOperations: Int
    let closeCalls: Int
    let isClosed: Bool
}

struct BenchmarkDeliveryGateSnapshot: Sendable {
    let started: Bool
    let finished: Bool
}

actor BenchmarkDeliveryGate {
    private var started = false
    private var released = false
    private var finished = false
    private var releaseWaiters = [CheckedContinuation<Void, Never>]()

    func markStarted() {
        started = true
    }

    func waitForRelease() async {
        if released { return }
        await withCheckedContinuation { continuation in
            if released {
                continuation.resume()
            } else {
                releaseWaiters.append(continuation)
            }
        }
    }

    func release() {
        guard released == false else { return }
        released = true
        let waiters = releaseWaiters
        releaseWaiters.removeAll(keepingCapacity: false)
        for waiter in waiters {
            waiter.resume()
        }
    }

    func markFinished() {
        finished = true
    }

    func snapshot() -> BenchmarkDeliveryGateSnapshot {
        BenchmarkDeliveryGateSnapshot(started: started, finished: finished)
    }
}

actor BenchmarkTransport: Transport {
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation
    private let oneWayDelayMilliseconds: Double
    private let bytesPerSecond: Int?
    private let dropOutboundOrdinals: Set<Int>
    private let interruptOutboundOrdinals: Set<Int>
    private var closed = false
    private var outboundAttempts = 0
    private var outboundFrames = [CapturedTransportFrame]()
    private var inboundFrames = [Data]()
    private var droppedMessages = 0
    private var interruptions = 0
    private var activeDelayedOperations = 0
    private var closeCalls = 0

    init(
        rttMilliseconds: Int = 0,
        bytesPerSecond: Int? = nil,
        dropOutboundOrdinals: Set<Int> = [],
        interruptOutboundOrdinals: Set<Int> = []
    ) {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream(
            bufferingPolicy: .bufferingOldest(4_096)
        )
        self.stream = stream
        self.continuation = continuation
        self.oneWayDelayMilliseconds = Double(rttMilliseconds) / 2.0
        self.bytesPerSecond = bytesPerSecond
        self.dropOutboundOrdinals = dropOutboundOrdinals
        self.interruptOutboundOrdinals = interruptOutboundOrdinals
    }

    deinit {
        continuation.finish()
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        guard closed == false else { throw TransportError.closed }
        outboundAttempts += 1
        let ordinal = outboundAttempts
        try await applyDelay(byteCount: data.count)
        try Task.checkCancellation()
        guard closed == false else { throw TransportError.closed }
        if interruptOutboundOrdinals.contains(ordinal) {
            interruptions += 1
            closed = true
            continuation.finish(throwing: TransportError.closed)
            throw TransportError.closed
        }
        if dropOutboundOrdinals.contains(ordinal) {
            droppedMessages += 1
            return
        }
        outboundFrames.append(CapturedTransportFrame(data: data, logicalClass: logicalClass))
    }

    func injectFromServer(
        _ data: Data,
        deliveryGate: BenchmarkDeliveryGate? = nil
    ) async throws {
        guard closed == false else { throw TransportError.closed }
        do {
            try await applyDelay(
                byteCount: data.count,
                deliveryGate: deliveryGate
            )
            if let deliveryGate {
                await deliveryGate.waitForRelease()
            }
            try Task.checkCancellation()
            guard closed == false else { throw TransportError.closed }
            inboundFrames.append(data)
            continuation.yield(data)
            if let deliveryGate {
                await deliveryGate.markFinished()
            }
        } catch {
            if let deliveryGate {
                await deliveryGate.markFinished()
            }
            throw error
        }
    }

    func close() {
        closeCalls += 1
        guard closed == false else { return }
        closed = true
        continuation.finish()
    }

    func framesSent() -> [CapturedTransportFrame] {
        outboundFrames
    }

    func snapshot() -> BenchmarkTransportSnapshot {
        BenchmarkTransportSnapshot(
            outboundAttempts: outboundAttempts,
            outboundMessages: outboundFrames.count,
            outboundBytes: outboundFrames.reduce(0) { $0 + $1.data.count },
            inboundMessages: inboundFrames.count,
            inboundBytes: inboundFrames.reduce(0) { $0 + $1.count },
            droppedMessages: droppedMessages,
            interruptions: interruptions,
            activeDelayedOperations: activeDelayedOperations,
            closeCalls: closeCalls,
            isClosed: closed
        )
    }

    private func applyDelay(
        byteCount: Int,
        deliveryGate: BenchmarkDeliveryGate? = nil
    ) async throws {
        let serializationMilliseconds = bytesPerSecond.map {
            Double(byteCount) / Double($0) * 1_000.0
        } ?? 0
        let total = oneWayDelayMilliseconds + serializationMilliseconds
        if total > 0 {
            activeDelayedOperations += 1
            if let deliveryGate {
                await deliveryGate.markStarted()
            }
            do {
                try await Task.sleep(for: .milliseconds(total))
            } catch {
                activeDelayedOperations -= 1
                throw error
            }
            activeDelayedOperations -= 1
        } else if let deliveryGate {
            await deliveryGate.markStarted()
        }
    }
}

func welcomeMessage(sessionID: String) -> SRUIMessage {
    var welcome = SRUIServerWelcome()
    welcome.coreVersion = SRUICoreVersion
    welcome.sessionID = sessionID
    welcome.requiredProfiles = [Profile.standardWidgetsV1.description]
    var message = SRUIMessage()
    message.serverWelcome = welcome
    return message
}

func terminalWelcomeMessage(
    sessionID: String,
    namespaceID: UInt32
) -> SRUIMessage {
    var terminalNamespace = Srui_Protocol_ExtensionNamespaceMapping()
    terminalNamespace.extensionUri = terminalProfileURI
    terminalNamespace.namespaceID = namespaceID

    var welcome = SRUIServerWelcome()
    welcome.coreVersion = SRUICoreVersion
    welcome.sessionID = sessionID
    welcome.requiredProfiles = [
        Profile.standardWidgetsV1.description,
        Profile.terminalV1.description,
    ]
    welcome.extensionNamespaces = [terminalNamespace]

    var message = SRUIMessage()
    message.serverWelcome = welcome
    return message
}

func resumeOKMessage(sessionID: String, lastProcessedEventSeq: UInt64 = 0) -> SRUIMessage {
    var resume = SRUIServerResumeOk()
    resume.sessionID = sessionID
    resume.replayFromRevision = 1
    resume.lastProcessedEventSeq = lastProcessedEventSeq
    var message = SRUIMessage()
    message.serverResumeOk = resume
    return message
}

func transactionMessage(_ transaction: Transaction) throws -> SRUIMessage {
    var message = SRUIMessage()
    message.transaction = transaction.toWire()
    return message
}

func framed(_ message: SRUIMessage) throws -> Data {
    try SRUIFraming.encodeFramed(message)
}

func acknowledgeMessage(
    _ event: Event,
    outbox: EventOutbox,
    sessionID: String,
    revision: Revision
) -> SRUIMessage {
    var acknowledgement = SRUIServerEventAck()
    acknowledgement.sessionID = sessionID
    acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
    acknowledgement.eventID = event.eventId.bytes
    acknowledgement.lastProcessedEventSeq = event.eventSeq
    acknowledgement.settledEventSeq = event.eventSeq
    acknowledgement.status = .processed
    acknowledgement.revisionAfterEffect = revision.value
    var message = SRUIMessage()
    message.serverEventAck = acknowledgement
    return message
}

@MainActor
func waitUntil(
    timeout: Duration = .seconds(5),
    condition: () async -> Bool
) async throws {
    let deadline = clock.now.advanced(by: timeout)
    while await condition() == false {
        guard clock.now < deadline else {
            throw BenchmarkFailure.message("timed out awaiting benchmark production-path state")
        }
        await Task.yield()
    }
}

func waitForRevision(
    _ revision: Revision,
    controller: SessionController
) async throws {
    try await waitUntil {
        controller.applier.lastAppliedRevision == revision
    }
}

struct CapturedEvent: Sendable {
    let semanticEvent: SemanticModel.Event

    var id: Data { semanticEvent.eventId.bytes }
    var sequence: UInt64 { semanticEvent.eventSeq }
    var observedRevision: UInt64 { semanticEvent.observedRevision.value }
    var nodeID: NodeId { semanticEvent.nodeId }
    var eventType: TypeRef { semanticEvent.eventType }
    var arguments: [PropertyRef: SemanticModel.Value] { semanticEvent.arguments }
    var editSeq: EditSeq? { semanticEvent.editSeq }
}

func capturedEvents(in transport: BenchmarkTransport) async throws -> [CapturedEvent] {
    let frames = await transport.framesSent()
    return try frames.compactMap { frame in
        let message = try SRUIFraming.decodeFramed(SRUIMessage.self, from: frame.data)
        guard case .event(let wireEvent)? = message.msg else { return nil }
        return CapturedEvent(
            semanticEvent: try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        )
    }
}
@MainActor
func startActiveSession(
    transport: BenchmarkTransport,
    renderer: AppKitRenderer,
    fixtureOperations: [SemanticModel.Operation],
    sessionID: String,
    outbox: EventOutbox = EventOutbox()
) async throws -> SessionController {
    let controller = SessionController(
        transport: transport,
        outbox: outbox,
        renderer: renderer
    )
    controller.attachRenderer(renderer)
    try await controller.start()
    try await transport.injectFromServer(
        try framed(welcomeMessage(sessionID: sessionID))
    )
    let initial = Transaction(baseRevision: Revision(0), operations: fixtureOperations)
    try await transport.injectFromServer(
        try framed(transactionMessage(initial))
    )
    try await waitForRevision(Revision(1), controller: controller)
    try await waitUntil {
        renderer.registry.handle(for: NodeId(1)) != nil
            && renderer.registry.handle(for: NodeId(14)) != nil
            && renderer.registry.handle(for: NodeId(16)) != nil
    }
    guard controller.isEventDispatchEnabled else {
        throw BenchmarkFailure.message("benchmark session did not enable event dispatch")
    }
    return controller
}
