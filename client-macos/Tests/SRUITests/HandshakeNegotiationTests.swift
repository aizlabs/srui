//
// HandshakeNegotiationTests.swift
// SRUITests
//
// Handshake negotiation and capability enforcement tests (§4 inv. 13, §15, §16, §22).
//

import Testing
import Foundation
import SemanticModel
import Protocol
import Session
import TransportSSH

@Suite("Handshake & Capability Negotiation Tests (§4 inv. 13, §15)")
struct HandshakeNegotiationTests {

    @Test("Successful handshake sets negotiated capabilities and enables active session")
    func successfulHandshake() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            clientCapabilities: [Profile.standardWidgetsV1, Profile.terminalV1]
        )

        try await controller.start()

        #expect(!controller.isHandshakeComplete)

        // 1. Server receives ClientHello
        let serverStream = serverTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        var receivedHello: SRUIClientHello?

        for try await chunk in serverStream {
            let messages = try streamDecoder.appendAndExtract(incoming: chunk)
            for msg in messages {
                if case .clientHello(let hello) = msg.msg {
                    receivedHello = hello
                    break
                }
            }
            if receivedHello != nil { break }
        }

        let hello = try #require(receivedHello)
        #expect(hello.coreVersion == "0.4.0")
        #expect(hello.profiles.contains("org.srui.standard-widgets/1"))
        #expect(hello.profiles.contains("org.srui.terminal/1"))

        // 2. Server sends ServerWelcome with required and optional profiles
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = "0.4.0"
        welcome.sessionID = "handshake-session-1"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.optionalProfiles = ["org.srui.terminal/1"]
        welcome.initialRevision = 0

        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        // 3. Client receives ServerWelcome and completes handshake
        try await AsyncTestSupport.eventually(description: "handshake completion") {
            controller.isHandshakeComplete
        }
        #expect(controller.negotiatedCapabilities == [Profile.standardWidgetsV1, Profile.terminalV1])
        #expect(!controller.isDiverged)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Mismatched required profile fails cleanly at handshake time (§4 inv. 13)")
    func mismatchedRequiredProfileFailsCleanly() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            clientCapabilities: [Profile.standardWidgetsV1]
        )

        let failurePromise = ManagedAtomic<SessionFailure?>(nil)
        controller.onFailure = { failure in
            failurePromise.store(failure)
        }

        try await controller.start()

        // 1. Drain ClientHello from server stream
        let serverStream = serverTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        for try await chunk in serverStream {
            let messages = try streamDecoder.appendAndExtract(incoming: chunk)
            if messages.contains(where: { if case .clientHello = $0.msg { return true } else { return false } }) {
                break
            }
        }

        // 2. Server sends ServerWelcome with required profile that client does not offer
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = "0.4.0"
        welcome.sessionID = "mismatch-session"
        welcome.requiredProfiles = ["org.srui.unsupported-feature/1"]
        welcome.optionalProfiles = []
        welcome.initialRevision = 0

        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        // 3. Client must fail with protocol violation, stop tracking, and close transport
        try await AsyncTestSupport.eventually(description: "handshake failure on profile mismatch") {
            controller.isDiverged && failurePromise.load() != nil
        }
        #expect(!controller.isHandshakeComplete)

        if let failure = failurePromise.load() {
            if case .protocolViolation(let msg) = failure {
                #expect(msg.contains("Capability negotiation failed"))
            } else {
                Issue.record("Expected protocolViolation, got \(failure)")
            }
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Transaction received before handshake completion is rejected as protocol violation")
    func transactionBeforeWelcomeIsRejected() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            clientCapabilities: [Profile.standardWidgetsV1]
        )

        let failurePromise = ManagedAtomic<SessionFailure?>(nil)
        controller.onFailure = { failure in
            failurePromise.store(failure)
        }

        try await controller.start()

        // Server sends Transaction immediately without sending ServerWelcome first
        let tx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [.createNode(id: NodeId(1), nodeType: .surface)]
        )
        var txMsg = SRUIMessage()
        txMsg.transaction = tx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(txMsg))

        try await AsyncTestSupport.eventually(description: "rejection of premature transaction") {
            controller.isDiverged && failurePromise.load() != nil
        }
        #expect(!controller.isHandshakeComplete)

        if let failure = failurePromise.load() {
            if case .protocolViolation(let msg) = failure {
                #expect(msg.contains("before handshake completed"))
            } else {
                Issue.record("Expected protocolViolation, got \(failure)")
            }
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Duplicate ServerWelcome after handshake completion is rejected")
    func duplicateWelcomeIsRejected() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            clientCapabilities: [Profile.standardWidgetsV1]
        )

        let failurePromise = ManagedAtomic<SessionFailure?>(nil)
        controller.onFailure = { failure in
            failurePromise.store(failure)
        }

        try await controller.start()

        // 1. Send first valid welcome
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = "0.4.0"
        welcome.sessionID = "session-1"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var msg1 = SRUIMessage()
        msg1.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(msg1))

        try await AsyncTestSupport.eventually(description: "initial handshake") {
            controller.isHandshakeComplete
        }

        // 2. Send second welcome during active session
        var msg2 = SRUIMessage()
        msg2.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(msg2))

        try await AsyncTestSupport.eventually(description: "rejection of duplicate welcome") {
            controller.isDiverged && failurePromise.load() != nil
        }

        await controller.stop()
        await serverTransport.close()
    }
}

private final class ManagedAtomic<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: T

    init(_ value: T) {
        self.value = value
    }

    func store(_ newValue: T) {
        lock.lock()
        defer { lock.unlock() }
        value = newValue
    }

    func load() -> T {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}
