//
// SavedEndpointRelaunchLiveTests.swift
// SRUITests
//
// PX-024: saved endpoints and reattachment after a client relaunch, against a live srtop behind a
// real localhost sshd and the real `srui-ssh-bridge`, all of which outlive the first client
// (§6.3, §17, §18, §18.2; D2 T23-T24, T38).
//
// A "launch" is a `ConnectionManager` with its own attempt factory over the one saved-connections
// file, and a "quit" is its `shutdown()`: nothing but that file passes from one launch to the
// next, exactly as across an app relaunch. The file holds the endpoint, its label, the session ID
// and the last revision as presentation metadata only. A relaunched client has no committed
// replica, so it attaches with CLIENT_HELLO and a fresh `client_instance_id`, and is ready only
// once the WELCOME catch-up snapshot holds the complete current state (§18, PX-024 round 2).
//
// Every fixture process is stopped from a dedicated `Thread`, never the main actor or the
// cooperative pool (PX-023 round 7), and every spawned process has its stdio set before `run()`.
// Missing fixture binaries fail; nothing is skipped.
//

import AppKit
import Foundation
@testable import ConnectionManager
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import TransportSSH

/// `SSHTransport` with its first outbound frames kept, so a test can read the handshake the client
/// actually put on the wire. Everything is forwarded unchanged.
private actor HandshakeRecordingTransport: Transport {
    private let inner: SSHTransport
    private var frames: [Data] = []

    init(_ inner: SSHTransport) {
        self.inner = inner
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        if frames.count < 4 { frames.append(data) }
        try await inner.send(data: data, logicalClass: logicalClass)
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        inner.receiveStream()
    }

    func close() async {
        await inner.close()
    }

    func acknowledgeReceived(byteCount: Int) async {
        await inner.acknowledgeReceived(byteCount: byteCount)
    }

    func firstMessage() throws -> SRUIMessage? {
        try frames.first.map { try decodeFramedMessage(from: $0) }
    }
}

/// One launch of the client: the production attempt path over the live fixture, with only the
/// test's key and known_hosts substituted, every attempt recorded and its transport's handshake
/// kept.
@MainActor
private final class AttemptLog {
    var contexts: [ConnectionSessionContext] = []
    var transports: [HandshakeRecordingTransport] = []
}

@MainActor
private final class ClientLaunch {
    let recorder: ContinuityRecorder
    let manager: ConnectionManager
    private let log: AttemptLog

    init(_ live: LiveSSHD, store url: URL) {
        let recorder = ContinuityRecorder()
        let log = AttemptLog()
        self.recorder = recorder
        self.log = log
        manager = ConnectionManager(store: SavedConnectionStore(url: url), attemptFactory: { request in
            #expect(request.configuration == ConnectionManager.sshConfiguration(for: request.connection))
            var configuration = request.configuration
            configuration.identityFile = live.userKey
            configuration.knownHostsFile = live.knownHosts
            configuration.connectTimeout = 10
            let transport = HandshakeRecordingTransport(SSHTransport(configuration: configuration))
            log.contexts.append(request.context)
            log.transports.append(transport)
            let generation = recorder.adopt(request.context)
            let holdsHandshake = recorder.holdsHandshake(of: generation)
            return RecordingConnectionAttempt(
                inner: ConnectionManager.makeSessionControllerAttempt(
                    request: request, transport: transport,
                    configureController: { controller in
                        guard holdsHandshake else { return }
                        controller.handshakeSendDidFinishForTesting = { await recorder.waitAtHandshake() }
                    }),
                generation: generation, recorder: recorder)
        })
    }

    var contexts: [ConnectionSessionContext] { log.contexts }
    var context: ConnectionSessionContext? { contexts.last }
    var applier: TransactionApplier? { contexts.last?.applier }
    var renderer: AppKitRenderer? { contexts.last?.renderer }
    var revision: UInt64 { applier?.lastAppliedRevision.value ?? 0 }

    /// The first message attempt `index` put on the wire.
    func firstMessage(of index: Int) async throws -> SRUIMessage? {
        guard log.transports.indices.contains(index) else { return nil }
        return try await log.transports[index].firstMessage()
    }

    func view<T>(_ id: UInt64, as _: T.Type = T.self) -> T? {
        renderer?.registry.handle(for: NodeId(id))?.view as? T
    }

    func text(_ id: UInt64) -> String { view(id, as: NSTextField.self)?.stringValue ?? "" }

    func nativeRows() -> [[String]] {
        guard let table = view(5, as: NSScrollView.self)?.documentView as? NSTableView else { return [] }
        return (0..<table.numberOfRows).map { row in
            (0..<table.tableColumns.count).map { column in
                (table.view(atColumn: column, row: row, makeIfNecessary: true) as? NSTextField)?.stringValue ?? ""
            }
        }
    }

    /// The row cells the replica holds, in published order.
    func modelRows() -> [[String]] {
        guard let model = applier?.store.getModel(ModelId(1)) else { return [] }
        return model.items.sorted { $0.key < $1.key }.map { item in
            guard case .list(let cells) = item.value.value else { return [] }
            return cells.map { cell in
                switch cell {
                case .unsignedInt(let number): return String(number)
                case .string(let text): return text
                default: return String(describing: cell)
                }
            }
        }
    }

    /// The stable item identities the replica holds, in published order.
    func itemIDs() -> [UInt64] {
        guard let model = applier?.store.getModel(ModelId(1)) else { return [] }
        return model.items.sorted { $0.key < $1.key }.map { $0.value.itemID.value }
    }

    /// Every node of the replica's tree, depth first from its roots.
    func tree() -> [Node] {
        guard let store = applier?.store else { return [] }
        var nodes: [Node] = []
        func visit(_ id: NodeId) {
            guard let node = store.getNode(id) else { return }
            nodes.append(node)
            node.orderedChildren.forEach(visit)
        }
        store.rootIDs.forEach(visit)
        return nodes
    }

    /// A complete, rendered explorer: rows in the replica, and the same rows in the native table.
    var showsRows: Bool {
        let rows = modelRows()
        return !rows.isEmpty && nativeRows() == rows
    }

    func closeWindows() {
        recorder.releaseHandshake()
        recorder.releaseReady()
        recorder.closeWindows()
        for context in contexts {
            for window in NSApplication.shared.windows
            where context.renderer.registry.handle(for: NodeId(1))?.window === window {
                window.orderOut(nil)
                window.close()
            }
        }
    }
}

/// The fixture processes a test owns, stopped together from a dedicated `Thread`: `shutDown()`
/// and `waitUntilExit()` block, which must never happen on the main actor or a cooperative thread.
private final class OwnedFixture: @unchecked Sendable {
    let live: LiveSSHD
    /// A restarted srtop on the fixture's socket, after the original was stopped.
    var replacement: Process?

    init(_ live: LiveSSHD) {
        self.live = live
    }

    func shutDownOffMainActor() {
        let thread = Thread { [self] in
            if let replacement {
                if replacement.isRunning { kill(replacement.processIdentifier, SIGINT) }
                replacement.waitUntilExit()
            }
            live.shutDown()
        }
        thread.name = "org.srui.tests.PX024.shutDown"
        thread.start()
    }
}

private let srtopBinary = "apps/srtop/target/debug/srtop"

private func storeDirectory() -> URL {
    FileManager.default.temporaryDirectory
        .appendingPathComponent("srui-px024-live-\(UUID().uuidString)", isDirectory: true)
}

/// Every launch a test made, so a failed expectation still closes their windows.
@MainActor
private final class Launches {
    private(set) var all: [ClientLaunch] = []

    func make(_ live: LiveSSHD, store url: URL) -> ClientLaunch {
        let launch = ClientLaunch(live, store: url)
        all.append(launch)
        return launch
    }

    func closeWindows() {
        all.forEach { $0.closeWindows() }
    }
}

/// What the first launch left behind: its saved entry, and the paused state srtop still holds.
private struct FirstLaunch {
    let id: SavedConnection.ID
    let session: String
    let identity: ClientInstanceId
    let frozenRevision: UInt64
    let frozenRows: [[String]]
    let frozenIDs: [UInt64]
}

/// Serialized: each test runs its own sshd, srtop and native windows on the one main actor.
@Suite("Saved endpoints reattach after a client relaunch (PX-024, §§17-18.2)", .serialized, .timeLimit(.minutes(3)))
@MainActor
struct SavedEndpointRelaunchLiveTests {
    static let samplingPauseID: UInt64 = 62
    static let samplingStatusID: UInt64 = 66

    private func waitFor(_ description: String, _ condition: @MainActor () -> Bool) async throws {
        try await AsyncTestSupport.eventually(timeout: .seconds(30), description: description, condition: condition)
    }

    /// The first launch: connect through the form, watch srtop update, pause it (this client's
    /// event 1, acknowledged) so its state stays put, and quit. The file then holds the entry.
    private func firstLaunchPausesAndQuits(
        _ live: LiveSSHD, store storeURL: URL, launches: Launches
    ) async throws -> FirstLaunch {
        let first = launches.make(live, store: storeURL)
        await first.manager.load()
        let id = try #require(first.manager.connect(ConnectDraft(
            label: "Explorer", host: "127.0.0.1", port: "\(live.port)", user: NSUserName())))
        try await waitFor("the first launch connected") { first.manager.status(for: id) == .connected && first.showsRows }
        let connectedAt = first.revision
        try await waitFor("srtop updates reached the first launch") { first.revision >= connectedAt + 3 }
        NativeActivation.click(try #require(first.view(Self.samplingPauseID, as: NSButton.self)))
        try await waitFor("the first launch paused srtop") {
            first.text(Self.samplingStatusID).hasPrefix("Paused at ")
                && first.view(Self.samplingPauseID, as: NSButton.self)?.title == "Resume view"
        }
        let outbox = try #require(first.context?.outbox)
        try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(30), description: "the pause acknowledged") {
            await outbox.lastAckedEventSeq == 1
        }
        // Let a scan already running when the pause landed return and be discarded.
        try await Task.sleep(for: .milliseconds(400))
        let state = FirstLaunch(
            id: id,
            session: try #require(first.manager.entries.first?.sessionID),
            identity: outbox.clientInstanceId,
            frozenRevision: first.revision,
            frozenRows: first.modelRows(),
            frozenIDs: first.itemIDs())
        await first.manager.shutdown()
        first.closeWindows()

        let saved = try #require(try await SavedConnectionStore(url: storeURL).load().first)
        #expect(saved.id == id && saved.label == "Explorer" && saved.host == "127.0.0.1" && saved.port == live.port)
        #expect(saved.sessionID == state.session)
        #expect(saved.lastKnownRevision == state.frozenRevision && state.frozenRevision > 0)
        return state
    }

    /// The CLIENT_HELLO attempt `index` of `launch` put on the wire: a cold relaunch attaches with
    /// the fresh identity, never a revision-0 resume (PX-024 round 2).
    private func expectHello(_ launch: ClientLaunch, attempt index: Int, identity: ClientInstanceId) async throws {
        let message = try await launch.firstMessage(of: index)
        guard case .clientHello(let hello)? = message?.msg else {
            Issue.record("attempt \(index + 1) did not attach with CLIENT_HELLO: \(String(describing: message?.msg))")
            return
        }
        #expect(hello.clientInstanceID == identity.bytes)
    }

    /// Quit after updates, relaunch from the saved entry alone, and reattach through two overlapping
    /// connects: at the moment the newer attempt is ready, its window already holds srtop's complete
    /// current state, from the WELCOME catch-up snapshot of a fresh attach with a fresh client
    /// identity. That identity's first event is processed (no sequence collision), and forgetting
    /// the entry afterwards leaves the session alive for every other client.
    @Test("a relaunch rebuilds srtop's complete current state with a fresh identity, and forgetting leaves the session alive")
    func relaunchRebuildsCompleteStateAndForgettingIsLocal() async throws {
        let live = try await LiveSSHD.launch(
            prefix: "px024-relaunch", knownHosts: .trusted,
            app: (srtopBinary, ["--fake-sequence", "--refresh-interval-ms", "200", "--journal-capacity", "1"],
                  FixtureBinaries.srtopRefusal))
        let fixture = OwnedFixture(live)
        defer { fixture.shutDownOffMainActor() }
        let directory = storeDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let storeURL = directory.appendingPathComponent("connections.json")
        _ = NSApplication.shared
        let launches = Launches()
        defer { launches.closeWindows() }

        let first = try await firstLaunchPausesAndQuits(live, store: storeURL, launches: launches)
        let id = first.id, session = first.session
        let saved = try #require(try await SavedConnectionStore(url: storeURL).load().first)

        // Relaunch: a new manager over the same file and nothing else. Two connects overlap: the
        // first is held after its handshake is on the wire, before it can read the answer, and the
        // newer one is held at ready so its window can be read before the chrome clears.
        let relaunched = launches.make(live, store: storeURL)
        await relaunched.manager.load()
        #expect(relaunched.manager.entries == [saved])
        #expect(relaunched.manager.status(for: id) == .unknown)
        relaunched.recorder.holdHandshake(of: 1)
        relaunched.manager.connect(id: id)
        try await waitFor("the first reattach held with its handshake sent") { relaunched.recorder.isHoldingHandshake }
        relaunched.recorder.holdReady(of: 2)
        relaunched.manager.connect(id: id)
        try await waitFor("the newer reattach held at ready") { relaunched.recorder.isHoldingReady }
        #expect(relaunched.recorder.events(of: 2) == ["connecting", "resynchronizing", "ready"])
        // Complete current state at ready, not an empty replica waiting for later deltas: srtop is
        // paused, so nothing has been published since the first launch quit.
        #expect(relaunched.recorder.readyRevisions[2] == first.frozenRevision, "ready only once the snapshot is in")
        #expect(relaunched.modelRows() == first.frozenRows)
        #expect(relaunched.itemIDs() == first.frozenIDs)
        // Drawing is paced by the renderer, not by commits (§12.2): the table catches up while held.
        try await waitFor("the snapshot drawn while held at ready") { relaunched.showsRows && relaunched.recorder.isHoldingReady }
        #expect(relaunched.manager.status(for: id) != .connected, "not live until ready is handled")
        relaunched.recorder.releaseReady()
        try await waitFor("the newer reattach ready") {
            relaunched.manager.status(for: id) == .connected && relaunched.showsRows
        }
        #expect(relaunched.contexts.count == 2 && relaunched.contexts[0] === relaunched.contexts[1],
                "both attempts of one launch share its one session context")
        let relaunchedOutbox = try #require(relaunched.context?.outbox)
        let relaunchedIdentity = relaunchedOutbox.clientInstanceId
        #expect(relaunchedIdentity != first.identity, "a relaunch never reuses the former client identity")
        try await expectHello(relaunched, attempt: 0, identity: relaunchedIdentity)
        try await expectHello(relaunched, attempt: 1, identity: relaunchedIdentity)
        #expect(relaunched.manager.entries.first?.sessionID == session)
        #expect(relaunched.manager.alert == nil, "the same session is no replacement")

        // The superseded attempt is released and runs to its end: it never becomes ready, and
        // nothing it reads reaches the status, the window or the replica.
        let readyRevision = relaunched.revision
        relaunched.recorder.releaseHandshake()
        try await waitFor("the superseded reattach ended") { relaunched.recorder.hasEnded(1) }
        #expect(!relaunched.recorder.events(of: 1).contains("ready"))
        try await Task.sleep(for: .milliseconds(300))
        #expect(relaunched.manager.status(for: id) == .connected)
        #expect(relaunched.manager.alert == nil, "\(relaunched.manager.alert?.message ?? "")")
        #expect(relaunched.revision >= readyRevision)
        #expect(relaunched.renderer?.registry.handle(for: NodeId(1))?.window?.subtitle == "")
        #expect(relaunched.modelRows() == first.frozenRows && relaunched.itemIDs() == first.frozenIDs)
        #expect(relaunched.text(Self.samplingStatusID).hasPrefix("Paused at "))

        // The whole tree equals what a fresh attach of another client receives.
        let observer = launches.make(live, store: directory.appendingPathComponent("observer.json"))
        let observerID = try #require(observer.manager.connect(ConnectDraft(
            label: "Observer", host: "127.0.0.1", port: "\(live.port)", user: NSUserName())))
        try await waitFor("an independent fresh attach") {
            observer.manager.status(for: observerID) == .connected && observer.showsRows
        }
        #expect(observer.manager.entries.first?.sessionID == session)
        try await waitFor("the relaunched replica equals a fresh attach") {
            relaunched.revision == observer.revision && relaunched.tree() == observer.tree()
                && relaunched.modelRows() == observer.modelRows() && relaunched.itemIDs() == observer.itemIDs()
        }
        #expect(relaunched.tree().count > 10)

        // The fresh identity's first event is sequence 1, and srtop processes it: Resume view.
        // Under the first launch's identity, sequence 1 would collide with its settled pause.
        NativeActivation.click(try #require(relaunched.view(Self.samplingPauseID, as: NSButton.self)))
        try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(30), description: "the resume acknowledged") {
            await relaunchedOutbox.lastAckedEventSeq == 1
        }
        try await waitFor("srtop sampling again for the relaunched client") {
            relaunched.text(Self.samplingStatusID).hasPrefix("Live") && relaunched.revision > readyRevision + 2
        }
        #expect(relaunched.manager.status(for: id) == .connected)

        // Forgetting is local: the entry and its file record go, nothing is sent, and the session,
        // still attached through this launch's open transport, keeps publishing to it and to the
        // observer. A copy of the file taken before the forget still reattaches to the same
        // incarnation.
        let copyURL = directory.appendingPathComponent("before-forget.json")
        try FileManager.default.copyItem(at: storeURL, to: copyURL)
        let forgottenAt = relaunched.revision
        relaunched.manager.remove(id: id)
        #expect(relaunched.manager.entries.isEmpty)
        await relaunched.manager.waitForPersistenceForTesting()
        #expect(try await SavedConnectionStore(url: storeURL).load().isEmpty)
        try await waitFor("the forgotten entry's session still publishing to it") {
            relaunched.revision > forgottenAt + 2
        }
        #expect(live.server?.isRunning == true)
        #expect(observer.manager.status(for: observerID) == .connected)
        #expect(observer.manager.entries.first?.sessionID == session)
        await relaunched.manager.shutdown()
        relaunched.closeWindows()

        let fromCopy = launches.make(live, store: copyURL)
        await fromCopy.manager.load()
        fromCopy.manager.connect(id: id)
        try await waitFor("the saved copy reattached") { fromCopy.manager.status(for: id) == .connected && fromCopy.showsRows }
        #expect(fromCopy.recorder.events(of: 1) == ["connecting", "resynchronizing", "ready"],
                "the same incarnation, not a replacement")
        #expect(fromCopy.manager.entries.first?.sessionID == session)
        #expect(fromCopy.manager.alert == nil)
        let copyIdentity = try #require(fromCopy.context?.outbox.clientInstanceId)
        try await expectHello(fromCopy, attempt: 0, identity: copyIdentity)
        #expect(copyIdentity != first.identity && copyIdentity != relaunchedIdentity)

        print("PX-024 native evidence: session=\(session) frozenRevision=\(first.frozenRevision) "
            + "rows=\(first.frozenRows.map { $0.prefix(2).joined(separator: " ") }) readyRevision=\(relaunched.recorder.readyRevisions[2] ?? 0) "
            + "identities=\(first.identity)/\(relaunchedIdentity)/\(copyIdentity) "
            + "attempts=\(relaunched.recorder.allEvents) copy=\(fromCopy.recorder.allEvents) "
            + "treeNodes=\(observer.tree().count)")
        await fromCopy.manager.shutdown()
        await observer.manager.shutdown()
    }

    /// PX-024 round 2 (F1): srtop with its DEFAULT journal still holds every commit since revision
    /// 0, which a revision-0 resume would replay after a `ready` at revision 0 (the window live over
    /// an empty, then historical, replica). The relaunch attaches with CLIENT_HELLO instead, so the
    /// moment it is ready its window already equals srtop's current state and a fresh attach's.
    @Test("with srtop's default journal, a relaunch is ready only once it shows the complete current state")
    func relaunchWithDefaultJournalIsReadyOnlyWithCompleteState() async throws {
        let live = try await LiveSSHD.launch(
            prefix: "px024-journal", knownHosts: .trusted,
            app: (srtopBinary, ["--fake-sequence", "--refresh-interval-ms", "200"], FixtureBinaries.srtopRefusal))
        let fixture = OwnedFixture(live)
        defer { fixture.shutDownOffMainActor() }
        let directory = storeDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let storeURL = directory.appendingPathComponent("connections.json")
        _ = NSApplication.shared
        let launches = Launches()
        defer { launches.closeWindows() }

        let first = try await firstLaunchPausesAndQuits(live, store: storeURL, launches: launches)
        let relaunched = launches.make(live, store: storeURL)
        await relaunched.manager.load()
        relaunched.recorder.holdReady(of: 1)
        relaunched.manager.connect(id: first.id)
        try await waitFor("the relaunch held at ready") { relaunched.recorder.isHoldingReady }
        #expect(relaunched.recorder.events(of: 1) == ["connecting", "resynchronizing", "ready"])
        #expect(relaunched.recorder.readyRevisions[1] == first.frozenRevision,
                "ready reported revision \(relaunched.recorder.readyRevisions[1] ?? 0), not the current \(first.frozenRevision)")
        #expect(relaunched.modelRows() == first.frozenRows)
        #expect(relaunched.itemIDs() == first.frozenIDs)
        // Drawing is paced by the renderer, not by commits (§12.2): the table catches up while held.
        try await waitFor("the snapshot drawn while held at ready") {
            relaunched.showsRows && relaunched.text(Self.samplingStatusID).hasPrefix("Paused at ")
        }

        // Still held at ready: the whole tree already equals a fresh attach of another client.
        let observer = launches.make(live, store: directory.appendingPathComponent("observer.json"))
        let observerID = try #require(observer.manager.connect(ConnectDraft(
            label: "Observer", host: "127.0.0.1", port: "\(live.port)", user: NSUserName())))
        try await waitFor("an independent fresh attach") {
            observer.manager.status(for: observerID) == .connected && observer.showsRows
        }
        try await waitFor("the relaunch at ready equals a fresh attach") {
            relaunched.revision == observer.revision && relaunched.tree() == observer.tree()
                && relaunched.modelRows() == observer.modelRows() && relaunched.itemIDs() == observer.itemIDs()
        }
        #expect(relaunched.recorder.isHoldingReady)
        #expect(relaunched.tree().count > 10)

        relaunched.recorder.releaseReady()
        try await waitFor("the relaunch connected") { relaunched.manager.status(for: first.id) == .connected }
        #expect(relaunched.manager.alert == nil)
        #expect(relaunched.manager.entries.first?.sessionID == first.session)
        let identity = try #require(relaunched.context?.outbox.clientInstanceId)
        #expect(identity != first.identity)
        try await expectHello(relaunched, attempt: 0, identity: identity)
        await relaunched.manager.waitForPersistenceForTesting()
        #expect(try await SavedConnectionStore(url: storeURL).load().first?.lastKnownRevision == first.frozenRevision)
        print("PX-024 native evidence (default journal): session=\(first.session) frozenRevision=\(first.frozenRevision) "
            + "readyRevision=\(relaunched.recorder.readyRevisions[1] ?? 0) attempts=\(relaunched.recorder.allEvents) "
            + "treeNodes=\(observer.tree().count)")
        await relaunched.manager.shutdown()
        await observer.manager.shutdown()
    }

    /// Quit, restart srtop (a new incarnation on the same endpoint), and relaunch: the saved
    /// session ID is stale. The WELCOME names another session, so the client reports a
    /// replacement in a blocking alert, rebuilds from the new incarnation's snapshot, and the saved
    /// entry is updated to the new session ID as soon as the WELCOME arrives, before ready.
    @Test("a relaunch against a restarted srtop surfaces the replacement and updates the saved session ID")
    func relaunchAgainstRestartedServerUpdatesTheSavedSession() async throws {
        let arguments = ["--fake-sequence", "--refresh-interval-ms", "200"]
        let live = try await LiveSSHD.launch(
            prefix: "px024-replaced", knownHosts: .trusted,
            app: (srtopBinary, arguments, FixtureBinaries.srtopRefusal))
        let fixture = OwnedFixture(live)
        defer { fixture.shutDownOffMainActor() }
        let directory = storeDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let storeURL = directory.appendingPathComponent("connections.json")
        _ = NSApplication.shared
        let launches = Launches()
        defer { launches.closeWindows() }

        let first = launches.make(live, store: storeURL)
        await first.manager.load()
        let id = try #require(first.manager.connect(ConnectDraft(
            label: "Explorer", host: "127.0.0.1", port: "\(live.port)", user: NSUserName())))
        try await waitFor("the first launch connected") { first.manager.status(for: id) == .connected && first.showsRows }
        let session = try #require(first.manager.entries.first?.sessionID)
        let firstIdentity = try #require(first.context?.outbox.clientInstanceId)
        await first.manager.shutdown()
        first.closeWindows()
        #expect(try await SavedConnectionStore(url: storeURL).load().first?.sessionID == session)

        // A new srtop on the same socket: the same endpoint, a new incarnation.
        let original = try #require(live.server)
        kill(original.processIdentifier, SIGTERM)
        try await waitFor("the original srtop stopped") { !original.isRunning }
        let socket = live.directory.appendingPathComponent("s.sock").path
        let replacement = Process()
        replacement.executableURL = LiveSSHD.root.appendingPathComponent(srtopBinary)
        replacement.arguments = ["--socket", socket] + arguments
        replacement.standardInput = FileHandle.nullDevice
        replacement.standardOutput = FileHandle.nullDevice
        replacement.standardError = FileHandle.nullDevice
        try replacement.run()
        fixture.replacement = replacement
        try await waitFor("the restarted srtop's socket") { FileManager.default.fileExists(atPath: socket) }

        let relaunched = launches.make(live, store: storeURL)
        await relaunched.manager.load()
        relaunched.recorder.holdReady(of: 1)
        relaunched.manager.connect(id: id)
        try await waitFor("the replacement decision, held at ready") {
            relaunched.recorder.isHoldingReady && relaunched.manager.alert?.kind == .sessionReplaced
        }
        #expect(relaunched.recorder.events(of: 1) == ["connecting", "replaced", "ready"])
        let replacementSession = try #require(relaunched.manager.entries.first?.sessionID)
        #expect(replacementSession != session)
        #expect(relaunched.manager.status(for: id) == .resynchronizing)
        let alert = try #require(relaunched.manager.alert)
        #expect(alert.title == "Session Replaced" && alert.message.contains(session) && alert.message.contains(replacementSession))
        let atDecision = try #require(try await SavedConnectionStore(url: storeURL).load().first)
        #expect(atDecision.sessionID == replacementSession, "the stale session ID is replaced before ready")
        #expect(atDecision.lastKnownRevision == 0)
        #expect(relaunched.renderer?.registry.handle(for: NodeId(1))?.window?.subtitle
            == ConnectionStatus.sessionReplacedWindowStatus)
        #expect(!relaunched.modelRows().isEmpty, "the new incarnation's snapshot is in at ready")
        // Drawing is paced by the renderer, not by commits (§12.2): the table catches up while held.
        try await waitFor("the snapshot drawn while held at ready") { relaunched.showsRows && relaunched.recorder.isHoldingReady }

        relaunched.recorder.releaseReady()
        try await waitFor("rebuilt from the new incarnation") {
            relaunched.manager.status(for: id) == .connected && relaunched.showsRows
        }
        let identity = try #require(relaunched.context?.outbox.clientInstanceId)
        #expect(identity != firstIdentity)
        try await expectHello(relaunched, attempt: 0, identity: identity)
        try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(30), description: "the ready revision saved") {
            let stored = try? await SavedConnectionStore(url: storeURL).load().first
            return stored?.sessionID == replacementSession && (stored?.lastKnownRevision ?? 0) > 0
        }
        print("PX-024 native evidence: replaced \(session) -> \(replacementSession) attempts=\(relaunched.recorder.allEvents)")
        await relaunched.manager.shutdown()
    }
}
