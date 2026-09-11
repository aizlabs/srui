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
        #expect(hello.coreVersion == SRUICoreVersion)
        #expect(hello.profiles.contains("org.srui.standard-widgets/1"))
        #expect(hello.profiles.contains("org.srui.terminal/1"))

        // 2. Server sends ServerWelcome with required and optional profiles
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "handshake-session-1"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.optionalProfiles = ["org.srui.terminal/1"]
        var terminalMapping = Srui_Protocol_ExtensionNamespaceMapping()
        terminalMapping.extensionUri = "org.srui.terminal/1"
        terminalMapping.namespaceID = 3
        welcome.extensionNamespaces = [terminalMapping]
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
    @Test("Default Terminal-capable client accepts a standard-only SERVER WELCOME")
    func defaultClientAcceptsStandardOnlyWelcome() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(transport: clientTransport)

        try await controller.start()

        let serverStream = serverTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        var receivedHello: SRUIClientHello?
        for try await chunk in serverStream {
            for message in try streamDecoder.appendAndExtract(incoming: chunk) {
                if case .clientHello(let hello) = message.msg {
                    receivedHello = hello
                    break
                }
            }
            if receivedHello != nil { break }
        }

        let hello = try #require(receivedHello)
        #expect(hello.profiles.contains("org.srui.standard-widgets/1"))
        #expect(hello.profiles.contains("org.srui.terminal/1"))

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "standard-only-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 0
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        try await AsyncTestSupport.eventually(description: "standard-only handshake completion") {
            controller.isHandshakeComplete
        }
        #expect(controller.negotiatedCapabilities == [Profile.standardWidgetsV1])
        #expect(!controller.isDiverged)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("SERVER WELCOME rejects negotiated optional Terminal without a namespace mapping")
    func optionalTerminalWithoutMappingFailsHandshake() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(transport: clientTransport)
        let failurePromise = ManagedAtomic<SessionFailure?>(nil)
        controller.onFailure = { failure in
            failurePromise.store(failure)
        }

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "terminal-without-mapping"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.optionalProfiles = ["org.srui.terminal/1"]
        welcome.initialRevision = 0
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        try await AsyncTestSupport.eventually(description: "missing Terminal mapping rejected") {
            controller.isDiverged && failurePromise.load() != nil
        }
        #expect(!controller.isHandshakeComplete)
        #expect(controller.negotiatedCapabilities == nil)

        if case .protocolViolation(let message)? = failurePromise.load() {
            #expect(message.contains("negotiated org.srui.terminal/1"))
            #expect(message.contains("omitted its namespace mapping"))
        } else {
            Issue.record("Expected protocolViolation, got \(String(describing: failurePromise.load()))")
        }

        await controller.stop()
        await serverTransport.close()
    }

    /// §15: core_version is part of the handshake, not decoration.
    /// §15: `core_version` is part of the handshake, not decoration.
    ///
    /// A proto3 string field that is absent decodes to `""`, so "omitted" and "empty" are the same
    /// wire state. Treating that as "unspecified, therefore compatible" would let a generated
    /// default authorize a session between peers that disagree about required semantics
    /// (§4 inv. 13), so it must fail closed alongside an explicitly incompatible version.
    @Test(
        "Incompatible or absent SERVER WELCOME core_version fails the handshake",
        arguments: ["", "1.0.0", "0.4.0", "garbage", "0"]
    )
    func incompatibleCoreVersionFailsHandshake(advertised: String) async throws {
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

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = advertised
        welcome.sessionID = "core-version-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 0

        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        try await AsyncTestSupport.eventually(description: "core version refusal") {
            controller.isDiverged && failurePromise.load() != nil
        }
        #expect(!controller.isHandshakeComplete)
        #expect(!controller.isEventDispatchEnabled)

        if case .protocolViolation(let message)? = failurePromise.load() {
            #expect(message.contains("core_version"))
        } else {
            Issue.record("Expected protocolViolation, got \(String(describing: failurePromise.load()))")
        }

        await controller.stop()
        await serverTransport.close()
    }

    /// The patch level is free: only `major.minor` decides compatibility (§15).
    @Test("A differing patch level still completes the handshake")
    func compatiblePatchLevelIsAccepted() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            clientCapabilities: [Profile.standardWidgetsV1]
        )

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = "0.5.99"
        welcome.sessionID = "core-version-patch"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 0

        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        try await AsyncTestSupport.eventually(description: "handshake completion") {
            controller.isHandshakeComplete
        }
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
        welcome.coreVersion = SRUICoreVersion
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
        welcome.coreVersion = SRUICoreVersion
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

    @Test("Client fails cleanly when server does not satisfy client required profiles")
    func clientRequiredProfilesMismatchFails() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            clientCapabilities: [Profile.standardWidgetsV1, Profile.terminalV1],
            requiredServerProfiles: [Profile.terminalV1] // Client requires terminal
        )

        let failurePromise = ManagedAtomic<SessionFailure?>(nil)
        controller.onFailure = { failure in
            failurePromise.store(failure)
        }

        try await controller.start()

        // 1. Drain ClientHello
        let serverStream = serverTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        for try await chunk in serverStream {
            let messages = try streamDecoder.appendAndExtract(incoming: chunk)
            if messages.contains(where: { if case .clientHello = $0.msg { return true } else { return false } }) {
                break
            }
        }

        // 2. Server sends ServerWelcome that ONLY provides standard-widgets (not terminal)
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "server-missing-client-req"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.optionalProfiles = []
        welcome.initialRevision = 0

        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        // 3. Client must fail because terminal was required by client
        try await AsyncTestSupport.eventually(description: "client required profile mismatch") {
            controller.isDiverged && failurePromise.load() != nil
        }
        #expect(!controller.isHandshakeComplete)

        if let failure = failurePromise.load() {
            if case .protocolViolation(let msg) = failure {
                #expect(msg.contains("Server does not satisfy client required profiles"))
            } else {
                Issue.record("Expected protocolViolation, got \(failure)")
            }
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Outbound event dispatch is rejected before handshake completion")
    func eventDispatchRejectedBeforeHandshake() async throws {
        let (clientTransport, _) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            clientCapabilities: [Profile.standardWidgetsV1]
        )

        #expect(!controller.isHandshakeComplete)
        await #expect(throws: SessionDispatchError.resumeNotConfirmed) {
            try await controller.sendActivate(nodeId: NodeId(1))
        }
    }

    @Test("Client-originated message received from server is rejected as protocol violation")
    func clientOriginatedMessageFromServerIsRejected() async throws {
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

        // Server sends ClientHello back to client
        var hello = SRUIClientHello()
        hello.coreVersion = "0.4.0"
        var helloMsg = SRUIMessage()
        helloMsg.clientHello = hello
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(helloMsg))

        try await AsyncTestSupport.eventually(description: "rejection of client message from server") {
            controller.isDiverged && failurePromise.load() != nil
        }

        if let failure = failurePromise.load() {
            if case .protocolViolation(let msg) = failure {
                #expect(msg.contains("client-originated handshake message"))
            } else {
                Issue.record("Expected protocolViolation, got \(failure)")
            }
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Client-originated model range request received from server is rejected as protocol violation")
    func clientModelRangeRequestFromServerIsRejected() async throws {
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

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "range-direction-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 0
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        try await AsyncTestSupport.eventually(description: "handshake completion") {
            controller.isHandshakeComplete
        }

        var request = SRUIClientModelRangeRequest()
        request.nodeID = 2
        request.modelID = 7
        request.startIndex = 0
        request.count = 8
        request.observedRevision = 0
        var requestMsg = SRUIMessage()
        requestMsg.clientModelRangeRequest = request
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(requestMsg))

        try await AsyncTestSupport.eventually(description: "rejection of client range request from server") {
            controller.isDiverged && failurePromise.load() != nil
        }

        if let failure = failurePromise.load() {
            if case .protocolViolation(let msg) = failure {
                #expect(msg.contains("model range request"))
            } else {
                Issue.record("Expected protocolViolation, got \(failure)")
            }
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Unsolicited SERVER RESUME_OK without CLIENT RESUME is a protocol violation")
    func unsolicitedResumeOkIsRejected() async throws {
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

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "spoofed-session"
        var resumeMsg = SRUIMessage()
        resumeMsg.serverResumeOk = resumeOk
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resumeMsg))

        try await AsyncTestSupport.eventually(description: "unsolicited RESUME_OK rejected") {
            controller.isDiverged && failurePromise.load() != nil
        }
        #expect(controller.isHandshakeComplete == false)
        #expect(controller.negotiatedCapabilities == nil)

        if let failure = failurePromise.load() {
            if case .protocolViolation(let msg) = failure {
                #expect(msg.contains("without an outstanding resume"))
            } else {
                Issue.record("Expected protocolViolation, got \(failure)")
            }
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Malformed SERVER WELCOME required profiles fail the handshake (§4 inv. 13)")
    func malformedRequiredProfilesFailHandshake() async throws {
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

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "bad-required"
        welcome.requiredProfiles = ["not-a-profile"]
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        try await AsyncTestSupport.eventually(description: "malformed required profiles fail closed") {
            controller.isDiverged && failurePromise.load() != nil
        }
        #expect(controller.isHandshakeComplete == false)

        if let failure = failurePromise.load() {
            if case .protocolViolation(let msg) = failure {
                #expect(msg.contains("required_profiles could not be parsed"))
            } else {
                Issue.record("Expected protocolViolation, got \(failure)")
            }
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("SERVER RESUME_OK restores negotiated capabilities for a resume-only session")
    func resumeOkPopulatesNegotiatedCapabilities() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let offered: CapabilitySet = [Profile.standardWidgetsV1, Profile.terminalV1]
        let controller = SessionController(
            transport: clientTransport,
            sessionId: "resume-session",
            clientCapabilities: offered
        )

        try await controller.start()
        #expect(controller.isHandshakeComplete == false)

        let serverStream = serverTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        var receivedResume: SRUIClientResume?
        for try await chunk in serverStream {
            for message in try streamDecoder.appendAndExtract(incoming: chunk) {
                if case .clientResume(let resume) = message.msg {
                    receivedResume = resume
                    break
                }
            }
            if receivedResume != nil { break }
        }
        let resume = try #require(receivedResume)
        #expect(resume.hasLimits)
        #expect(resume.limits.maxResourceSize == 50 * 1024 * 1024)
        #expect(resume.knownResourceHashes.isEmpty)
        #expect(resume.coreVersion == SRUICoreVersion)
        #expect(resume.profiles.contains("org.srui.standard-widgets/1"))
        #expect(resume.profiles.contains("org.srui.terminal/1"))

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = "org.srui.terminal/1"
        mapping.namespaceID = 3
        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "resume-session"
        resumeOk.requiredProfiles = [
            "org.srui.standard-widgets/1",
            "org.srui.terminal/1",
        ]
        resumeOk.extensionNamespaces = [mapping]
        var resumeMsg = SRUIMessage()
        resumeMsg.serverResumeOk = resumeOk
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resumeMsg))

        try await AsyncTestSupport.eventually(description: "resume handshake completion") {
            controller.isHandshakeComplete
        }
        #expect(controller.negotiatedCapabilities == offered)
        #expect(controller.isDiverged == false)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("SERVER RESUME_OK after a completed WELCOME handshake is a protocol violation")
    func strayResumeOkAfterWelcomeIsRejected() async throws {
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
        try await serverTransport.send(
            data: try SRUIFraming.encodeFramed(HandshakeFixtures.welcomeMessage(sessionId: "session-1"))
        )
        try await AsyncTestSupport.eventually(description: "initial handshake") {
            controller.isHandshakeComplete
        }

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-1"
        var resumeMsg = SRUIMessage()
        resumeMsg.serverResumeOk = resumeOk
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resumeMsg))

        try await AsyncTestSupport.eventually(description: "stray RESUME_OK rejected") {
            controller.isDiverged && failurePromise.load() != nil
        }

        await controller.stop()
        await serverTransport.close()
    }

    @Test("WELCOME with a nonzero initial revision applies the following snapshot before enabling dispatch")
    func helloCatchUpSnapshotIsApplied() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            clientCapabilities: [Profile.standardWidgetsV1]
        )

        try await controller.start()

        var welcome = HandshakeFixtures.welcomeMessage(sessionId: "hello-bootstrap").serverWelcome
        welcome.initialRevision = 1
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        try await AsyncTestSupport.eventually(description: "handshake complete while awaiting snapshot") {
            controller.isHandshakeComplete
        }
        #expect(applier.lastAppliedRevision == .initial)

        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [.createNode(id: NodeId(1), nodeType: .surface)]
        )
        var snapshotMsg = SRUIMessage()
        snapshotMsg.transaction = snapshot.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMsg))

        try await AsyncTestSupport.eventually(description: "hello catch-up snapshot applied") {
            applier.lastAppliedRevision == Revision(1) && controller.isEventDispatchEnabled
        }
        #expect(controller.isDiverged == false)
        #expect(controller.negotiatedCapabilities == [Profile.standardWidgetsV1])
        _ = try await controller.sendActivate(nodeId: NodeId(1))

        await controller.stop()
        await serverTransport.close()
    }
}
