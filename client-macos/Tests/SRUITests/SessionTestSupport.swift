//
// SessionTestSupport.swift
// SRUITests
//
// Shared deterministic scheduling and wire-observation support for session tests.
//

import Foundation
import Protocol
import SemanticModel
@testable import Session
import Testing
import TransportSSH

/// A deterministic test seam that records arrival and blocks until explicitly released.
///
/// Cancellation is intentionally ignored while paused: the test controls the interleaving and
/// releases the gate after asserting the state that exists while the operation is suspended.
actor AsyncGate {
    private var hasArrived = false
    private var isReleased = false
    private var arrivalWaiters: [CheckedContinuation<Void, Never>] = []
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []

    func pause() async {
        hasArrived = true
        let waiters = arrivalWaiters
        arrivalWaiters.removeAll()
        for waiter in waiters {
            waiter.resume()
        }

        guard isReleased == false else { return }
        await withCheckedContinuation { continuation in
            if isReleased {
                continuation.resume()
            } else {
                releaseWaiters.append(continuation)
            }
        }
    }

    func pauseAfterPublish() async {
        await pause()
    }

    func waitUntilPaused() async {
        guard hasArrived == false else { return }
        await withCheckedContinuation { continuation in
            if hasArrived {
                continuation.resume()
            } else {
                arrivalWaiters.append(continuation)
            }
        }
    }

    func release() {
        guard isReleased == false else { return }
        isReleased = true
        let waiters = releaseWaiters
        releaseWaiters.removeAll()
        for waiter in waiters {
            waiter.resume()
        }
    }
}

/// Drains one framed transport stream and retains projected messages in arrival order.
actor WireMessageCollector<Element: Sendable> {
    typealias Project = @Sendable (SRUIMessage) -> Element?

    private let project: Project
    private var collected: [Element] = []
    private var task: Task<Void, Never>?

    init(project: @escaping Project) {
        self.project = project
    }

    func start(draining transport: any Transport) {
        guard task == nil else { return }
        let stream = transport.receiveStream()
        let project = self.project
        task = Task { [weak self] in
            var decoder = SRUIMessageStreamDecoder()
            do {
                for try await chunk in stream {
                    for message in try decoder.appendAndExtract(incoming: chunk) {
                        guard let value = project(message) else { continue }
                        await self?.append(value)
                    }
                }
            } catch {
                // Test teardown closes the transport; retain everything collected before closure.
            }
        }
    }

    private func append(_ value: Element) {
        collected.append(value)
    }

    private func count() -> Int {
        collected.count
    }

    func wait(forAtLeast expectedCount: Int, timeout: TimeInterval = 5) async -> [Element] {
        do {
            try await AsyncTestSupport.eventuallyAsync(
                timeout: .seconds(timeout),
                description: "\(expectedCount) projected wire messages"
            ) {
                self.count() >= expectedCount
            }
        } catch {
            Issue.record("\(error)")
        }
        return collected
    }

    func stop() {
        task?.cancel()
        task = nil
    }
}

extension WireMessageCollector where Element == SRUIMessage {
    init() {
        self.init(project: { $0 })
    }
}

extension WireMessageCollector where Element == Event {
    init() {
        self.init(project: { message in
            guard case .event(let wire) = message.msg else { return nil }
            return try? ProtocolDecoder().validateAndConvertEvent(wire: wire)
        })
    }

    func events() -> [Event] {
        collected
    }

    func eventCount() -> Int {
        collected.count
    }
}

typealias EventCollector = WireMessageCollector<Event>
typealias ResyncWireCollector = WireMessageCollector<SRUIMessage>
typealias LiveRenderGate = AsyncGate
typealias TextLifecycleGate = AsyncGate
typealias LifecycleRaceGate = AsyncGate

actor SessionFailureRecorder {
    private(set) var first: SessionFailure?

    func record(_ failure: SessionFailure) {
        if first == nil {
            first = failure
        }
    }

    func wait(timeout: TimeInterval = 5) async -> SessionFailure? {
        do {
            try await AsyncTestSupport.eventuallyAsync(
                timeout: .seconds(timeout),
                description: "first session failure"
            ) {
                self.first != nil
            }
        } catch {
            return nil
        }
        return first
    }
}

typealias ResyncFailureRecorder = SessionFailureRecorder
