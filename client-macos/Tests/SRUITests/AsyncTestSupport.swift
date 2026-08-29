import Foundation

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
