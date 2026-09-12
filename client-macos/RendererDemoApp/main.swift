//
// main.swift
// RendererDemoApp
//
// Entry point for the SRUI macOS client and its transport demos.
//

import AppKit
import ConnectionManager
import RendererAppKit
import SemanticModel
import Session
import SwiftUI
import TransportSSH

@main
struct RendererDemoApp {
    @MainActor
    static func main() {
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)
        SRUIApplicationMenu.install(on: application)

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
        } else if args.contains("--demo") {
            RendererDemo.run()
        } else {
            runConnectionManager()
        }
    }

    @MainActor
    private static func runConnectionManager() -> Never {
        let application = NSApplication.shared

        let manager = ConnectionManager()
        let delegate = ConnectionManagerApplicationDelegate(manager: manager)
        ConnectionManagerApplicationDelegate.retained = delegate
        application.delegate = delegate
        application.finishLaunching()
        delegate.start()
        application.run()
        fatalError("NSApplication run loop terminated")
    }

    @MainActor
    private static func runLiveSession(transport: any Transport) -> Never {
        RendererDiagnostics.log("Launching SRUI Client with live transport...")
        let application = NSApplication.shared

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
private final class ConnectionManagerApplicationDelegate: NSObject, NSApplicationDelegate {
    static var retained: ConnectionManagerApplicationDelegate?

    private let manager: ConnectionManager
    private let window: NSWindow
    private var startupTask: Task<Void, Never>?
    private var isStopping = false

    init(manager: ConnectionManager) {
        self.manager = manager

        let hostingController = NSHostingController(
            rootView: ConnectionManagerView(manager: manager)
        )
        window = NSWindow(contentViewController: hostingController)
        window.title = "SRUI Connections"
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.setContentSize(NSSize(width: 720, height: 460))
        window.center()
        window.isReleasedWhenClosed = false

        super.init()
    }

    func start() {
        guard startupTask == nil else { return }
        startupTask = Task { [weak self] in
            guard let self else { return }
            await manager.load()
            guard !Task.isCancelled else { return }
            startupTask = nil
            window.makeKeyAndOrderFront(nil)
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
    }

    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows _: Bool
    ) -> Bool {
        guard startupTask == nil else { return true }
        if window.isMiniaturized {
            window.deminiaturize(nil)
        }
        window.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !isStopping else { return .terminateNow }
        isStopping = true
        let startupTask = self.startupTask
        self.startupTask = nil

        Task {
            startupTask?.cancel()
            await startupTask?.value
            await manager.shutdown()
            NSApplication.shared.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

@MainActor
private final class LiveApplicationDelegate: NSObject, NSApplicationDelegate {
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
