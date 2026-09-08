//
// TransactionRateLimiter.swift
// Session
//
// Per-session semantic transaction ingress backpressure (§12.2, §26).
//

import Dispatch
import Foundation

/// Finite local policy for semantic transaction admission (§26).
public struct TransactionRateLimits: Equatable, Sendable {
    public static let defaultSustainedTransactionsPerSecond: UInt64 = 120
    public static let defaultBurstCapacity: UInt64 = 240

    public static let standard = TransactionRateLimits(
        validatedSustainedTransactionsPerSecond: defaultSustainedTransactionsPerSecond,
        burstCapacity: defaultBurstCapacity
    )

    public let sustainedTransactionsPerSecond: UInt64
    public let burstCapacity: UInt64

    /// Returns nil for zero or unrepresentably large limits instead of silently disabling the
    /// bound.
    public init?(sustainedTransactionsPerSecond: UInt64, burstCapacity: UInt64) {
        guard sustainedTransactionsPerSecond > 0,
              burstCapacity > 0,
              sustainedTransactionsPerSecond <= UInt64.max / TransactionRateLimiter.unitsPerToken,
              burstCapacity <= UInt64.max / TransactionRateLimiter.unitsPerToken else {
            return nil
        }
        self.init(
            validatedSustainedTransactionsPerSecond: sustainedTransactionsPerSecond,
            burstCapacity: burstCapacity
        )
    }

    private init(
        validatedSustainedTransactionsPerSecond: UInt64,
        burstCapacity: UInt64
    ) {
        self.sustainedTransactionsPerSecond = validatedSustainedTransactionsPerSecond
        self.burstCapacity = burstCapacity
    }
}

enum TransactionAdmission: Equatable {
    case admitted
    case wait(nanoseconds: UInt64)
}

/// Integer token bucket. Credit is measured in token-nanoseconds so refill remains deterministic
/// and no floating-point rounding can accidentally admit traffic over the configured ceiling.
struct TransactionRateLimiter: Sendable {
    static let unitsPerToken: UInt64 = 1_000_000_000

    private let limits: TransactionRateLimits
    private let capacityUnits: UInt64
    private var availableUnits: UInt64
    private var lastRefillUptimeNanoseconds: UInt64?

    init(limits: TransactionRateLimits = .standard) {
        self.limits = limits
        self.capacityUnits = limits.burstCapacity * Self.unitsPerToken
        self.availableUnits = capacityUnits
        self.lastRefillUptimeNanoseconds = nil
    }

    mutating func reset() {
        availableUnits = capacityUnits
        lastRefillUptimeNanoseconds = nil
    }

    mutating func admission(atUptimeNanoseconds now: UInt64) -> TransactionAdmission {
        refill(atUptimeNanoseconds: now)
        guard availableUnits >= Self.unitsPerToken else {
            let deficit = Self.unitsPerToken - availableUnits
            let rate = limits.sustainedTransactionsPerSecond
            return .wait(nanoseconds: (deficit + rate - 1) / rate)
        }
        availableUnits -= Self.unitsPerToken
        return .admitted
    }

    private mutating func refill(atUptimeNanoseconds now: UInt64) {
        defer {
            if let previous = lastRefillUptimeNanoseconds {
                if now > previous {
                    lastRefillUptimeNanoseconds = now
                }
            } else {
                lastRefillUptimeNanoseconds = now
            }
        }
        guard let previous = lastRefillUptimeNanoseconds, now > previous else { return }

        let elapsed = now - previous
        let (refill, refillOverflow) = elapsed.multipliedReportingOverflow(
            by: limits.sustainedTransactionsPerSecond
        )
        let boundedRefill = refillOverflow ? capacityUnits : min(refill, capacityUnits)
        let (refilled, additionOverflow) = availableUnits.addingReportingOverflow(boundedRefill)
        availableUnits = additionOverflow ? capacityUnits : min(refilled, capacityUnits)
    }
}

/// Session-owned ingress gate. Waiting here keeps transport acknowledgement withheld, propagating
/// backpressure to both live traffic and journal replay without silently dropping either.
actor TransactionIngressGate {
    private var limiter: TransactionRateLimiter

    init(limits: TransactionRateLimits = .standard) {
        self.limiter = TransactionRateLimiter(limits: limits)
    }

    func reset() {
        limiter.reset()
    }

    func waitForAdmission() async throws {
        while true {
            switch limiter.admission(
                atUptimeNanoseconds: DispatchTime.now().uptimeNanoseconds
            ) {
            case .admitted:
                return
            case .wait(let nanoseconds):
                try await Task.sleep(for: .nanoseconds(Int64(nanoseconds)))
            }
        }
    }
}
