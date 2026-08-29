import Foundation
import Protocol

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
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)

        while condition() == false {
            guard clock.now < deadline else {
                throw AsyncTestTimeout(description: "Timed out waiting for \(description)")
            }
            await Task.yield()
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
        welcome.coreVersion = "0.4.0"
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
