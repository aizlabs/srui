//
// main.swift
// RendererDemoApp
//
// Entry point for SRUI macOS Client application (§22).
// Supports standalone demo mode or connecting to a live session daemon via socket.
//

import AppKit
import Session
import TransportSSH
import RendererAppKit
import SemanticModel

@main
struct RendererDemoApp {
    @MainActor
    static func main() {
        let args = CommandLine.arguments

        if let sshIndex = args.firstIndex(of: "--ssh"), sshIndex + 1 < args.count {
            let host = args[sshIndex + 1]
            var port: UInt16?
            var user: String?
            var subsystem = "srui"
            var identity: String?
            var knownHosts: String?
            var batchMode = true
            var connectTimeout: TimeInterval = 30.0

            if let pIdx = args.firstIndex(of: "--port"), pIdx + 1 < args.count {
                port = UInt16(args[pIdx + 1])
            }
            if let uIdx = args.firstIndex(of: "--user"), uIdx + 1 < args.count {
                user = args[uIdx + 1]
            }
            if let sIdx = args.firstIndex(of: "--subsystem"), sIdx + 1 < args.count {
                subsystem = args[sIdx + 1]
            }
            if let iIdx = args.firstIndex(of: "--identity"), iIdx + 1 < args.count {
                identity = args[iIdx + 1]
            }
            if let kIdx = args.firstIndex(of: "--known-hosts"), kIdx + 1 < args.count {
                knownHosts = args[kIdx + 1]
            }
            if args.contains("--interactive") {
                batchMode = false
            }
            if let tIdx = args.firstIndex(of: "--connect-timeout"), tIdx + 1 < args.count,
               let seconds = TimeInterval(args[tIdx + 1]) {
                connectTimeout = seconds
            }

            let config = SSHConfiguration(
                host: host,
                port: port,
                user: user,
                subsystem: subsystem,
                identityFile: identity,
                knownHostsFile: knownHosts,
                batchMode: batchMode,
                connectTimeout: connectTimeout
            )
            runLiveSession(transport: SSHTransport(configuration: config))
        } else if let socketIndex = args.firstIndex(of: "--socket"), socketIndex + 1 < args.count {
            let socketPath = args[socketIndex + 1]
            runLiveSession(transport: UnixSocketTransport(socketPath: socketPath))
        } else if let tcpIndex = args.firstIndex(of: "--tcp"), tcpIndex + 2 < args.count {
            let host = args[tcpIndex + 1]
            let port = UInt16(args[tcpIndex + 2]) ?? 8080
            runLiveSession(transport: TCPSocketTransport(host: host, port: port))
        } else {
            RendererDemo.run()
        }
    }

    @MainActor
    private static func runLiveSession(transport: any Transport) -> Never {
        RendererDiagnostics.log("Launching SRUI Client with live transport...")
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)

        let renderer = AppKitRenderer()
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: transport,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        // `NSApplication.delegate` is a weak reference, so the delegate must be owned somewhere that
        // outlives this scope. A plain local can be released right after its last use, leaving the
        // app with a nil delegate and no termination callbacks at all.
        let delegate = LiveApplicationDelegate(controller: controller, renderer: renderer)
        LiveApplicationDelegate.retained = delegate
        application.delegate = delegate
        application.finishLaunching()
        delegate.start()
        application.run()
        fatalError("NSApplication run loop terminated")
    }
}

@MainActor
private final class LiveApplicationDelegate: NSObject, NSApplicationDelegate {
    /// Strong owner for the delegate, which `NSApplication` only references weakly.
    static var retained: LiveApplicationDelegate?

    private let controller: SessionController
    private let renderer: AppKitRenderer
    private var isStopping = false

    init(controller: SessionController, renderer: AppKitRenderer) {
        self.controller = controller
        self.renderer = renderer
    }

    func start() {
        NSApplication.shared.activate(ignoringOtherApps: true)
        Task {
            do {
                try await controller.start()
            } catch {
                RendererDiagnostics.log("SessionController start failed: \(error)")
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    /// Shuts the session down before the process exits.
    ///
    /// This must not block the main thread waiting on a `Task`: an unstructured `Task` created here
    /// inherits `MainActor` isolation, so blocking the main thread would prevent it from ever
    /// starting and deadlock termination. `.terminateLater` keeps the run loop alive instead, and
    /// `reply(toApplicationShouldTerminate:)` resumes the quit once cleanup finishes.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !isStopping else { return .terminateNow }
        isStopping = true

        Task {
            await controller.stop()
            NSApplication.shared.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}
