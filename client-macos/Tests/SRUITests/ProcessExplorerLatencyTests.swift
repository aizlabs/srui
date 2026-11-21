//
// ProcessExplorerLatencyTests.swift
// SRUITests
//
// PX-025: local interaction latency versus remote result latency, and wire bytes versus renderer
// cadence, for the Process Explorer in the unchanged generic client (D1 §12.2, §31.3, §31.4;
// D2 T28-T29, T34).
//
// The transport path is the real one: the generic client, OpenSSH, the srui SSH bridge and srtop,
// with PX-025's userspace delay relay (`apps/srtop/demo`, `srtop-demo relay`) between the bridge
// and srtop's socket. The relay adds RTT/2 in each direction; it needs no root, `tc`, `dnctl` or
// `pfctl`. Nothing in the client or srtop knows it is there.
//
// * Local interactions - typing into the filter field, moving its caret, scrolling rows the
//   client already holds - must show without any RTT wait at 0/100/300/600 ms.
// * The server-dependent result of the same typing - the committed filter status - must arrive
//   no sooner than one RTT and track it.
// * For one deterministic fake state stream, the bytes on the wire must not depend on how often
//   the renderer presents.
//

import AppKit
import Darwin
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
import Testing
import TransportSSH

@testable import Session

@Suite(.serialized)
struct ProcessExplorerLatencyTests {
    private static let filterInputID: UInt64 = 26
    private static let filterStatusID: UInt64 = 27
    private static let rtts: [Int] = [0, 100, 300, 600]
    /// The worst a local interaction may take on a loaded test machine; a 100 ms RTT alone is
    /// twice this.
    private static let localBudget = Duration.milliseconds(50)
    /// How far a local interaction's median may move from the 0 ms one.
    private static let localDriftBudget = Duration.milliseconds(25)

    private struct RTTEvidence {
        let rtt: Int
        var typing: [Duration] = []
        var caret: [Duration] = []
        var scrolling: [Duration] = []
        var remote: [Duration] = []
        /// Keystrokes whose server result was still outstanding when the local echo was complete.
        var heldAtLocalCompletion = 0
        var sentDuringCaretAndScroll = 0
    }

    private static func median(_ samples: [Duration]) -> Duration {
        let sorted = samples.sorted()
        return sorted.isEmpty ? .zero : sorted[sorted.count / 2]
    }

    private static func milliseconds(_ duration: Duration) -> Double {
        let (seconds, attoseconds) = duration.components
        return Double(seconds) * 1000 + Double(attoseconds) / 1e15
    }

    private static func list(_ samples: [Duration]) -> String {
        "[" + samples.map { String(format: "%.1f", milliseconds($0)) }.joined(separator: ", ") + "]"
    }

    /// Local feedback never waits for the server; the server's answer to the same typing waits
    /// one RTT. Both, at every RTT D1 §31.4 names.
    @Test(.timeLimit(.minutes(4)))
    @MainActor
    func localInteractionGainsNoRTTWhileRemoteResultsAreDelayedByIt() async throws {
        let harness = try await LatencyHarness.launch(srtopArguments: ["--fake-repeated-names"])
        var evidence: [RTTEvidence] = []
        do {
            for rtt in Self.rtts {
                let relay = try await harness.startRelay(rttMilliseconds: rtt)
                do {
                    evidence.append(try await Self.measure(rtt: rtt, harness: harness))
                } catch {
                    await relay.stop()
                    throw error
                }
                await relay.stop()
            }
        } catch {
            await harness.shutDown()
            throw error
        }
        await harness.shutDown()

        let baseline = try #require(evidence.first)
        for run in evidence {
            let rtt = Duration.milliseconds(run.rtt)
            for (kind, samples, base) in [
                ("typing", run.typing, baseline.typing),
                ("caret", run.caret, baseline.caret),
                ("scrolling", run.scrolling, baseline.scrolling),
            ] {
                #expect(samples.count == 5, "\(kind) at \(run.rtt) ms")
                for sample in samples {
                    #expect(sample < Self.localBudget,
                            "\(kind) at \(run.rtt) ms RTT took \(sample): local feedback waited")
                }
                let drift = Self.median(samples) - Self.median(base)
                #expect(drift < Self.localDriftBudget,
                        "\(kind) at \(run.rtt) ms RTT: median moved \(drift) from 0 ms RTT")
            }
            if run.rtt > 0 {
                // The causal proof: when the typed character was on screen, the server's result
                // for it had not arrived, so the echo cannot have waited for it.
                #expect(run.heldAtLocalCompletion == run.typing.count,
                        "at \(run.rtt) ms RTT only \(run.heldAtLocalCompletion) of \(run.typing.count) keystrokes showed before their result")
            }
            #expect(run.remote.count == 5)
            for sample in run.remote {
                #expect(sample >= rtt, "a remote result at \(run.rtt) ms RTT arrived after \(sample)")
            }
            // Every sample is at least one RTT (above); and not held far past it. A shift against
            // the 0 ms median is not asserted: under a loaded parallel run that median alone can
            // exceed 100 ms, while the per-sample lower bound holds whatever the load.
            #expect(Self.median(run.remote) < rtt + .milliseconds(1_000))
            print("PX-025 native latency evidence: rtt \(run.rtt) ms; typing \(Self.list(run.typing)); caret \(Self.list(run.caret)); scrolling \(Self.list(run.scrolling)); remote filter result \(Self.list(run.remote)); held at local completion \(run.heldAtLocalCompletion)/\(run.typing.count); bytes sent during caret+scroll \(run.sentDuringCaretAndScroll)")
        }
    }

    @MainActor
    private static func measure(rtt: Int, harness: LatencyHarness) async throws -> RTTEvidence {
        _ = NSApplication.shared
        let tap = WireTap(inner: SSHTransport(configuration: harness.configuration))
        let renderer = AppKitRenderer()
        let applier = TransactionApplier()
        let controller = SessionController(transport: tap, applier: applier, outbox: EventOutbox(),
                                           renderer: renderer)
        controller.attachRenderer(renderer)
        renderer.textEditingSession.debounceNanoseconds = 0
        try await controller.start()
        var run = RTTEvidence(rtt: rtt)
        do {
            try await AsyncTestSupport.eventually(timeout: .seconds(30), description: "the explorer at \(rtt) ms RTT") {
                applier.lastAppliedRevision.value >= 1
                    && controller.isEventDispatchEnabled
                    && renderer.registry.handle(for: NodeId(filterStatusID)) != nil
                    && ((renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView) != nil
            }
            let window = try #require(renderer.registry.handle(for: NodeId(1))?.window)
            defer { window.orderOut(nil); window.close() }
            SurfacePresentation.forHostApplication().present(window)
            window.contentView?.layoutSubtreeIfNeeded()
            let table = try #require((renderer.registry.handle(for: NodeId(5))?.view as? NSScrollView)?.documentView as? NSTableView)
            let filterHandle = try #require(renderer.registry.handle(for: NodeId(filterInputID)))
            let filterField = try #require(filterHandle.view as? NSTextField)
            let filterAdapter = try #require(filterHandle.textAdapter)
            let status = try #require(renderer.registry.handle(for: NodeId(filterStatusID))?.view as? NSTextField)

            // Start from no filter (an earlier run's text is the session's), all 60 rows held.
            if !filterField.stringValue.isEmpty {
                filterField.stringValue = ""
                filterAdapter.notifyTextDidChangeForTests()
                filterAdapter.notifyEndEditingForTests()
            }
            try await AsyncTestSupport.eventually(timeout: .seconds(30), description: "no filter, 60 rows at \(rtt) ms") {
                status.stringValue.contains("none") && table.numberOfRows == 60
                    && modelRowCount(applier) == 60
            }

            // Scrolling rows the client already holds: no row is fetched, so nothing waits.
            let rows = modelPIDs(applier)
            let sentBefore = tap.sentBytes
            for target in [59, 0, 45, 10, 30] {
                let clock = ContinuousClock()
                let start = clock.now
                table.scrollRowToVisible(target)
                table.layoutSubtreeIfNeeded()
                let visible = NSLocationInRange(target, table.rows(in: table.visibleRect))
                let cell = (table.view(atColumn: 0, row: target, makeIfNecessary: true) as? NSTextField)?.stringValue
                run.scrolling.append(start.duration(to: clock.now))
                #expect(visible, "row \(target) is in the viewport at \(rtt) ms")
                #expect(cell == rows[target], "row \(target) shows its PID at \(rtt) ms")
            }

            // Typing into the filter field through its own key handling.
            #expect(window.makeFirstResponder(filterField))
            let editor = try #require(window.firstResponder as? NSTextView, "the field editor owns input")
            var typed = ""
            for (character, keyCode) in [("w", UInt16(13)), ("o", 31), ("r", 15), ("k", 40), ("e", 14)] {
                typed += character
                let expected = "\"\(typed)\""
                let event = try keyEvent(character, keyCode: keyCode, in: window)
                let clock = ContinuousClock()
                let start = clock.now
                editor.keyDown(with: event)
                let echoed = editor.string == typed && filterField.stringValue == typed
                let local = start.duration(to: clock.now)
                if !status.stringValue.contains(expected) { run.heldAtLocalCompletion += 1 }
                run.typing.append(local)
                #expect(echoed, "the field shows \(typed) at \(rtt) ms")
                let deadline = clock.now + .seconds(15)
                while !status.stringValue.contains(expected) {
                    guard clock.now < deadline else {
                        throw AsyncTestTimeout(description: "the filter result for \(typed) at \(rtt) ms; status \(status.stringValue)")
                    }
                    try await Task.sleep(for: .milliseconds(1))
                }
                run.remote.append(start.duration(to: clock.now))
            }

            // Moving the caret: purely local.
            for _ in 0..<5 {
                let before = editor.selectedRange().location
                let event = try keyEvent(String(Character(UnicodeScalar(NSLeftArrowFunctionKey)!)),
                                         [.numericPad, .function], keyCode: 123, in: window)
                let clock = ContinuousClock()
                let start = clock.now
                editor.keyDown(with: event)
                let moved = editor.selectedRange().location == before - 1
                run.caret.append(start.duration(to: clock.now))
                #expect(moved, "the caret moved left at \(rtt) ms")
            }
            run.sentDuringCaretAndScroll = tap.sentBytes - sentBefore - tap.sentBytesForEvents(after: sentBefore)
            window.makeFirstResponder(nil)
        } catch {
            await controller.stop()
            throw error
        }
        await controller.stop()
        return run
    }

    /// §12.2, §31.3: two clients of one srtop session, one presenting every commit at once and
    /// one presenting slowly and in batches, receive the same bytes for every revision of the same
    /// deterministic stream and send the same bytes; only their present passes differ.
    @Test(.timeLimit(.minutes(2)))
    @MainActor
    func wireBytesForTheSameFakeStreamAreIndependentOfRendererCadence() async throws {
        let harness = try await LatencyHarness.launch(srtopArguments: ["--fake-sequence", "--refresh-interval-ms", "150"])
        do {
            let relay = try await harness.startRelay(rttMilliseconds: 100)
            do {
                try await Self.compareCadences(harness: harness)
            } catch {
                await relay.stop()
                throw error
            }
            await relay.stop()
        } catch {
            await harness.shutDown()
            throw error
        }
        await harness.shutDown()
    }

    @MainActor
    private static func compareCadences(harness: LatencyHarness) async throws {
        _ = NSApplication.shared
        final class Cadence: @unchecked Sendable {
            let name: String
            /// Renders per present pass, and how long each render takes beyond the renderer's own.
            let batch: Int
            let renderCost: Duration
            let lock = NSLock()
            var renders = 0
            var presents = 0
            init(name: String, batch: Int, renderCost: Duration) {
                self.name = name
                self.batch = batch
                self.renderCost = renderCost
            }
        }
        let cadences = [Cadence(name: "every commit", batch: 1, renderCost: .zero),
                        Cadence(name: "slow batches", batch: 4, renderCost: .milliseconds(40))]
        var clients: [(Cadence, WireTap, SessionController, TransactionApplier, AppKitRenderer)] = []
        for cadence in cadences {
            let tap = WireTap(inner: SSHTransport(configuration: harness.configuration))
            let renderer = AppKitRenderer()
            let applier = TransactionApplier()
            let controller = SessionController(transport: tap, applier: applier, outbox: EventOutbox(),
                                               renderer: renderer)
            controller.attachRenderer(renderer)
            controller.rendererDidRenderInterceptorForTesting = { [weak renderer] in
                if cadence.renderCost > .zero { try? await Task.sleep(for: cadence.renderCost) }
                let present = cadence.lock.withLock { () -> Bool in
                    cadence.renders += 1
                    return cadence.renders % cadence.batch == 0
                }
                guard present else { return }
                await MainActor.run {
                    renderer?.registry.handle(for: NodeId(1))?.window?.contentView?.layoutSubtreeIfNeeded()
                }
                cadence.lock.withLock { cadence.presents += 1 }
            }
            clients.append((cadence, tap, controller, applier, renderer))
        }
        do {
            for (_, _, controller, _, _) in clients { try await controller.start() }
            try await AsyncTestSupport.eventually(timeout: .seconds(30), description: "both clients attached") {
                clients.allSatisfy { $0.3.lastAppliedRevision.value >= 1 && $0.2.isEventDispatchEnabled }
            }
            let sentAtReady = clients.map { $0.1.sentBytes }
            let start = clients.map { $0.3.lastAppliedRevision.value }.max() ?? 0
            // Twelve more commits of the script: more than two cycles, failed scan included.
            try await AsyncTestSupport.eventually(timeout: .seconds(30), description: "twelve commits on both clients") {
                clients.allSatisfy { $0.3.lastAppliedRevision.value >= start + 12 }
            }
            let end = start + 12
            let streams = clients.map { $0.1.transactions(in: (start + 1)...end) }
            let sent = clients.enumerated().map { $0.element.1.sentBytes - sentAtReady[$0.offset] }
            for (index, stream) in streams.enumerated() {
                #expect(stream.count == 12, "\(cadences[index].name) received every revision: \(stream.map(\.newRevision))")
            }
            #expect(streams[0].map(\.newRevision) == streams[1].map(\.newRevision))
            #expect(streams[0].map(\.bytes) == streams[1].map(\.bytes),
                    "the same revisions arrived as different bytes at different render cadences")
            #expect(sent[0] == sent[1], "the clients sent \(sent) bytes while only rendering")
            let presents = cadences.map { cadence in cadence.lock.withLock { cadence.presents } }
            let renders = cadences.map { cadence in cadence.lock.withLock { cadence.renders } }
            // Not vacuous: the cadence really differed.
            #expect(presents[0] > presents[1], "present passes \(presents) did not differ")
            let bytes = streams.map { $0.reduce(0) { $0 + $1.bytes.count } }
            print("PX-025 render cadence evidence: revisions \(start + 1)...\(end); bytes received \(bytes) (identical per revision: \(streams[0].map(\.bytes) == streams[1].map(\.bytes))); bytes sent after ready \(sent); renders \(renders); present passes \(presents)")
        } catch {
            for (_, _, controller, _, _) in clients { await controller.stop() }
            throw error
        }
        for (_, _, controller, _, renderer) in clients {
            renderer.registry.handle(for: NodeId(1))?.window?.orderOut(nil)
            await controller.stop()
        }
    }

    @MainActor
    private static func modelRowCount(_ applier: TransactionApplier) -> Int {
        applier.store.getModel(ModelId(1))?.items.count ?? 0
    }

    /// The PID cell of every row the client holds, in published order.
    @MainActor
    private static func modelPIDs(_ applier: TransactionApplier) -> [String] {
        guard let model = applier.store.getModel(ModelId(1)) else { return [] }
        return model.items.sorted { $0.key < $1.key }.map { item in
            guard case .list(let cells) = item.value.value, let first = cells.first else { return "" }
            switch first {
            case .unsignedInt(let number): return String(number)
            case .string(let text): return text
            default: return String(describing: first)
            }
        }
    }

    @MainActor
    private static func keyEvent(
        _ characters: String, _ modifiers: NSEvent.ModifierFlags = [], keyCode: UInt16, in window: NSWindow
    ) throws -> NSEvent {
        try #require(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: modifiers, timestamp: 0,
            windowNumber: window.windowNumber, context: nil, characters: characters,
            charactersIgnoringModifiers: characters, isARepeat: false, keyCode: keyCode
        ))
    }
}

/// srtop on a private socket, the relay socket path, and an ephemeral sshd whose srui subsystem is
/// the real bridge pointed at the relay. Processes are awaited on dedicated threads or through
/// termination handlers, never on the main actor or the cooperative pool.
private final class LatencyHarness: @unchecked Sendable {
    let temp: URL
    let server: Process
    let sshd: Process
    let configuration: SSHConfiguration
    let srtopSocket: String
    let relaySocket: String
    let relayBinary: URL

    private init(temp: URL, server: Process, sshd: Process, configuration: SSHConfiguration,
                 srtopSocket: String, relaySocket: String, relayBinary: URL) {
        self.temp = temp
        self.server = server
        self.sshd = sshd
        self.configuration = configuration
        self.srtopSocket = srtopSocket
        self.relaySocket = relaySocket
        self.relayBinary = relayBinary
    }

    static func launch(srtopArguments: [String]) async throws -> LatencyHarness {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let srtop = root.appendingPathComponent("apps/srtop/target/debug/srtop")
        let bridge = root.appendingPathComponent("server-rust/target/debug/srui-ssh-bridge")
        let relay = root.appendingPathComponent("apps/srtop/demo/target/debug/srtop-demo")
        for binary in [srtop, bridge, relay] {
            try #require(FileManager.default.isExecutableFile(atPath: binary.path),
                         "Run bash apps/srtop/test.sh to build \(binary.path); this test must not soft-skip")
        }
        try await FixtureBinaries.absorbFirstLaunchAssessment(srtop.path, arguments: FixtureBinaries.srtopRefusal)
        try await FixtureBinaries.absorbFirstLaunchAssessment(bridge.path, arguments: FixtureBinaries.bridgeRefusal)
        try await FixtureBinaries.absorbFirstLaunchAssessment(relay.path, arguments: [])
        let temp = try TestFixtureDirectory.make(prefix: "px025")
        let srtopSocket = temp.appendingPathComponent("s.sock").path
        let relaySocket = temp.appendingPathComponent("r.sock").path
        let hostKey = temp.appendingPathComponent("host_key").path
        let userKey = temp.appendingPathComponent("user_key").path
        let authorizedKeys = temp.appendingPathComponent("authorized_keys")
        let knownHosts = temp.appendingPathComponent("known_hosts").path
        let config = temp.appendingPathComponent("sshd_config")
        let server = Process()
        server.executableURL = srtop
        server.arguments = ["--socket", srtopSocket] + srtopArguments
        server.standardInput = FileHandle.nullDevice
        server.standardOutput = FileHandle.nullDevice
        server.standardError = FileHandle.nullDevice
        do {
            // `generateEd25519Key` waits for ssh-keygen: on a dedicated thread, not the pool.
            let generated = ManagedAtomic<String?>(nil)
            await onThread {
                do {
                    try SSHTestSupport.generateEd25519Key(at: hostKey)
                    try SSHTestSupport.generateEd25519Key(at: userKey)
                } catch {
                    generated.store(String(describing: error))
                }
            }
            if let failure = generated.load() { throw AsyncTestTimeout(description: failure) }
            try Data(contentsOf: URL(fileURLWithPath: "\(userKey).pub")).write(to: authorizedKeys)
            let port = SSHTestSupport.findFreePort()
            try SSHTestSupport.writeKnownHosts(port: port, hostPublicKeyPath: hostKey, to: knownHosts)
            // The subsystem's socket is the relay's, so every byte crosses the delay line.
            try """
            Port \(port)
            ListenAddress 127.0.0.1
            HostKey \(hostKey)
            AuthorizedKeysFile \(authorizedKeys.path)
            StrictModes no
            UsePAM no
            PidFile \(temp.appendingPathComponent("sshd.pid").path)
            Subsystem srui \(bridge.path) \(relaySocket)
            """.write(to: config, atomically: true, encoding: .utf8)
            try server.run()
            try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "srtop private socket") {
                FileManager.default.fileExists(atPath: srtopSocket)
            }
            let sshd = try SSHTestSupport.launchSSHD(configPath: config.path, hostKeyPath: hostKey, port: port)
            do {
                try await SSHTestSupport.waitForPort(port: port)
            } catch {
                await onThread { SSHTestSupport.terminate(sshd) }
                throw error
            }
            return LatencyHarness(
                temp: temp, server: server, sshd: sshd,
                configuration: SSHConfiguration(
                    host: "127.0.0.1", port: port, subsystem: "srui",
                    identityFile: userKey, knownHostsFile: knownHosts,
                    strictHostKeyChecking: .yes, batchMode: true, connectTimeout: 10.0),
                srtopSocket: srtopSocket, relaySocket: relaySocket, relayBinary: relay)
        } catch {
            if server.isRunning { kill(server.processIdentifier, SIGINT) }
            await onThread { if server.processIdentifier != 0 { server.waitUntilExit() } }
            TestFixtureDirectory.release(temp)
            throw error
        }
    }

    /// Starts `srtop-demo relay` on the relay socket at `rttMilliseconds`. Its standard input is
    /// its lifeline: closing it stops the relay, which removes its socket.
    func startRelay(rttMilliseconds: Int) async throws -> RelayProcess {
        let process = Process()
        process.executableURL = relayBinary
        process.arguments = ["relay", "--listen", relaySocket, "--upstream", srtopSocket,
                             "--rtt-ms", String(rttMilliseconds)]
        let lifeline = Pipe()
        process.standardInput = lifeline
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        let relay = RelayProcess(process: process, lifeline: lifeline)
        try process.run()
        let socket = relaySocket
        try await AsyncTestSupport.eventually(timeout: .seconds(10), description: "the relay at \(rttMilliseconds) ms") {
            FileManager.default.fileExists(atPath: socket)
        }
        return relay
    }

    func shutDown() async {
        let server = self.server
        let sshd = self.sshd
        if server.isRunning { kill(server.processIdentifier, SIGINT) }
        await onThread {
            server.waitUntilExit()
            SSHTestSupport.terminate(sshd)
        }
        TestFixtureDirectory.release(temp)
    }
}

private final class RelayProcess: @unchecked Sendable {
    let process: Process
    let lifeline: Pipe

    init(process: Process, lifeline: Pipe) {
        self.process = process
        self.lifeline = lifeline
    }

    /// Closes the lifeline and waits for the relay to remove its socket and exit; a relay that
    /// has not exited in 5 s is killed (it is this test's own unreaped child).
    func stop() async {
        try? lifeline.fileHandleForWriting.close()
        let process = self.process
        await onThread {
            let deadline = Date().addingTimeInterval(5)
            while process.isRunning && Date() < deadline { usleep(10_000) }
            if process.isRunning { kill(process.processIdentifier, SIGKILL) }
            process.waitUntilExit()
        }
    }
}

/// Runs `body` on a dedicated thread and resumes when it returns, so a blocking wait never runs
/// on the main actor or the cooperative pool.
private func onThread(_ body: @escaping @Sendable () -> Void) async {
    await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
        let thread = Thread {
            body()
            continuation.resume()
        }
        thread.start()
    }
}

/// A pass-through transport that counts every byte the client sends and splits a copy of every
/// chunk it receives into framed messages, keeping each transaction's exact bytes. Chunks and
/// acknowledgements are forwarded unchanged, so flow control stays the inner transport's.
private final class WireTap: Transport, @unchecked Sendable {
    struct Frame: Sendable {
        let baseRevision: UInt64
        let newRevision: UInt64
        let bytes: Data
    }

    private let inner: any Transport
    private let lock = NSLock()
    private var pending = Data()
    private var frames: [Frame] = []
    private var sent = 0
    /// Bytes of sent frames that carried an `Event`, by the running total they followed.
    private var eventSends: [(after: Int, bytes: Int)] = []

    init(inner: any Transport) {
        self.inner = inner
    }

    var sentBytes: Int { lock.withLock { sent } }

    /// The bytes of event frames sent after the running total was `after`.
    func sentBytesForEvents(after: Int) -> Int {
        lock.withLock { eventSends.filter { $0.after >= after }.reduce(0) { $0 + $1.bytes } }
    }

    /// Every incremental transaction received whose new revision is in `range`, in arrival order.
    func transactions(in range: ClosedRange<UInt64>) -> [Frame] {
        lock.withLock { frames.filter { $0.baseRevision != 0 && range.contains($0.newRevision) } }
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        try await inner.send(data: data, logicalClass: logicalClass)
        let isEvent: Bool
        if let message = try? decodeFramedMessage(from: data), case .event = message.msg {
            isEvent = true
        } else {
            isEvent = false
        }
        lock.withLock {
            if isEvent { eventSends.append((sent, data.count)) }
            sent += data.count
        }
    }

    func close() async {
        await inner.close()
    }

    func acknowledgeReceived(byteCount: Int) async {
        await inner.acknowledgeReceived(byteCount: byteCount)
    }

    func receiveStream() -> AsyncThrowingStream<Data, Error> {
        let inner = self.inner
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await chunk in inner.receiveStream() {
                        self.observe(chunk)
                        continuation.yield(chunk)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func observe(_ chunk: Data) {
        lock.lock()
        defer { lock.unlock() }
        pending.append(chunk)
        while true {
            var length = 0
            var shift = 0
            var prefix = 0
            var complete = false
            for byte in pending.prefix(10) {
                prefix += 1
                length |= Int(byte & 0x7f) << shift
                shift += 7
                if byte & 0x80 == 0 { complete = true; break }
            }
            guard complete, pending.count >= prefix + length else { return }
            let frame = Data(pending.prefix(prefix + length))
            pending = Data(pending.dropFirst(prefix + length))
            if let message = try? decodeFramedMessage(from: frame), case .transaction(let wire) = message.msg {
                frames.append(Frame(baseRevision: wire.baseRevision, newRevision: wire.newRevision, bytes: frame))
            }
        }
    }
}
