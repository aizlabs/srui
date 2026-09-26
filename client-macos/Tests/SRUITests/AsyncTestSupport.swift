import Foundation
import Protocol
import Session

struct AsyncTestTimeout: Error, CustomStringConvertible {
    let description: String
}

enum AsyncTestSupport {
    @MainActor
    static func eventually(
        timeout: Duration = .seconds(2),
        description: String,
        condition: @MainActor () -> Bool
    ) async throws {
        try await eventuallyAsync(timeout: timeout, description: description) {
            condition()
        }
    }

    /// Poll interval. Deliberately a real sleep rather than `Task.yield()`: most conditions here
    /// are `@MainActor`, and a tight yield loop on the main actor keeps re-enqueuing cooperative
    /// work, so the main thread never goes idle and never services its run-loop sources, while
    /// burning a whole core that CI (3 cores) needs for the work being awaited.
    static let pollInterval = Duration.milliseconds(10)

    static func eventuallyAsync(
        isolation: isolated (any Actor)? = #isolation,
        timeout: Duration = .seconds(2),
        description: String,
        condition: () async -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)

        while await condition() == false {
            guard clock.now < deadline else {
                throw AsyncTestTimeout(description: "Timed out waiting for \(description)")
            }
            try await Task.sleep(for: pollInterval)
        }
    }
}

enum HandshakeFixtures {
    static func welcomeMessage(
        sessionId: String = "test-session",
        requiredProfiles: [String] = ["org.srui.standard-widgets/1"],
        optionalProfiles: [String] = []
    ) -> SRUIMessage {
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = sessionId
        welcome.requiredProfiles = requiredProfiles
        welcome.optionalProfiles = optionalProfiles
        var msg = SRUIMessage()
        msg.serverWelcome = welcome
        return msg
    }
}

final class ManagedAtomic<T: Sendable>: @unchecked Sendable {
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
