import Foundation
import Protocol
import Session

struct AsyncTestTimeout: Error, CustomStringConvertible {
    let description: String
}

extension Duration {
    /// Budget for an `AsyncTestSupport.eventually` condition that spans a full transport round
    /// trip: a frame pushed into a `PipeTransport`/unix socket/SSH subsystem, read back by the
    /// session's reader task, negotiated, applied on the `TransactionApplier`, and rendered on the
    /// main actor.
    ///
    /// The helper's 2-second default stays as it is on purpose — a short default is what keeps a
    /// genuinely hung condition from spending the whole suite's time budget — but it is not enough
    /// for a multi-step round trip on a loaded 3-core CI runner. Observed on GitHub's hosted
    /// runner: `SessionControllerTerminalTests` "A recreated process cold-resumes Terminal after
    /// authoritative negotiation" failed with `Timed out waiting for cold Terminal replay mounted`
    /// while passing locally and on a rerun of the identical tree, i.e. the deadline expired, not
    /// the product.
    ///
    /// A deadline is only an upper bound: on a healthy run every one of these conditions is met in
    /// milliseconds and nothing here is ever asserted against, so raising it costs no wall time and
    /// weakens no assertion. `.seconds(10)` matches what the live SSH and socket integration suites
    /// in this target already use for the same shape.
    ///
    /// "Does a real transport feed this condition?" was the wrong test for which call sites need
    /// it, and suites driving an in-memory harness were left on the 2-second default on that
    /// basis. What actually matters is whether the condition is satisfied by **another task making
    /// progress**: swift-testing runs all 62 suites of this target at once against a cooperative
    /// pool no wider than the machine's cores, so a continuation can simply not be scheduled
    /// inside two seconds no matter how little work it has to do. Measured, in a full local run on
    /// an idle machine: `ConnectionManagerTests` "a failed first connection discards its session
    /// context" timed out after 3.99s waiting for `failed draft removed` - an entry removed by a
    /// MainActor hop off an in-memory harness, with no I/O anywhere in it.
    static let roundTrip = Duration.seconds(10)
}

enum AsyncTestSupport {
    /// `Duration.roundTrip` for the `TimeInterval`-based pollers that predate it.
    ///
    /// A suite with its own `waitUntil` helper never saw the shared budget, which is how
    /// `SessionRobustnessTests` kept a 2-second deadline on a condition that is not a transport
    /// round trip at all but *another task reaching a suspension point*. swift-testing runs the
    /// whole target concurrently and the cooperative pool is only as wide as the machine's cores,
    /// so under a full-suite run that task can simply not be scheduled inside two seconds:
    /// "stop() during handshake send tears down transport and allows restart" failed on
    /// `await transport.isSendBlocked` in a full local run on an idle machine, and passes in
    /// isolation. One number, referenced from both helpers, so neither can drift.
    static let roundTripSeconds: TimeInterval = 10

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

    /// Poll interval. A real sleep rather than `Task.yield()`, which busy-waits: every condition
    /// polled here depends on I/O or an actor hop, so re-checking thousands of times per second
    /// buys nothing and costs CPU that the awaited work needs (CI runners have 3 cores).
    ///
    /// This is hygiene, not a fix for any known failure: with `Task.yield()` restored, the two
    /// tests this helper was suspected of breaking still pass. In particular it is *not*
    /// justified by run-loop starvation — sampling a parked test helper shows 0% CPU and a main
    /// thread idle in `CFRunLoopRun`/`mach_msg`, so that hang is a lost wakeup, not contention.
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
