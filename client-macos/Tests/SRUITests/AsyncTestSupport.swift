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
            await Task.yield()
        }
    }
}

/// Cross-thread rendezvous a test can *await* instead of blocking on.
///
/// Why this exists, rather than `DispatchSemaphore` or `NSCondition`: a blocking wait inside a
/// test body parks the cooperative thread that is running that test. swift-testing schedules the
/// whole suite concurrently, and the cooperative pool is only as wide as the machine's active
/// cores, so on a small machine a handful of blocking waits parks every thread in the pool -
/// after which the work that would have signalled them can never be scheduled and the run
/// deadlocks outright.
///
/// Measured on the 3-core hosted CI runner: two tests blocked in `GatedFailingSink`'s
/// `NSCondition` plus one blocked `DispatchSemaphore.wait` held all three cooperative threads,
/// the main thread sat idle in `mach_msg`, and swift-testing reported 0 of 574 tests before the
/// watchdog killed the run (`723 started, 149 reported`, exit 137). The same commit passed
/// 725/725 with `--no-parallel`, and passes in parallel on a 12-core machine - which is why this
/// class of bug reaches CI unnoticed.
///
/// `signal()` is safe to call from a thread that must not suspend (a `Thread` body, a
/// `DispatchQueue`, a synchronous transport callback); only the *waiter* becomes async.
final class AsyncTestSignal: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private var waiters: [(threshold: Int, continuation: CheckedContinuation<Void, Never>)] = []

    /// How many times `signal()` has been called. For negative assertions ("still not signalled").
    var signalCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    /// Records one signal and resumes every waiter whose threshold it reaches. Never blocks.
    func signal() {
        lock.lock()
        count += 1
        let ready = waiters.filter { $0.threshold <= count }
        waiters.removeAll { $0.threshold <= count }
        lock.unlock()
        for waiter in ready {
            waiter.continuation.resume()
        }
    }

    /// Bounded form: suspends until the threshold is reached, and reports whether it was, so a
    /// caller can keep asserting on the outcome the way a `DispatchSemaphore` timeout let it.
    func waitOrTimeout(until threshold: Int = 1, timeout: Duration = .seconds(5)) async -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while signalCount < threshold {
            guard clock.now < deadline else { return false }
            try? await Task.sleep(for: .milliseconds(5))
        }
        return true
    }

    /// Suspends - never blocks a thread - until `signal()` has been called `threshold` times.
    func wait(until threshold: Int = 1) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            lock.lock()
            if count >= threshold {
                lock.unlock()
                continuation.resume()
                return
            }
            waiters.append((threshold, continuation))
            lock.unlock()
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
