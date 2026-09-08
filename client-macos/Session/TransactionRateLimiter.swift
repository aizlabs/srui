//
// TransactionRateLimiter.swift
// Session
//
// Per-session semantic transaction rate enforcement (§12.2, §26).
//

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

    /// Returns nil for zero or unrepresentably large limits instead of silently disabling or
    /// weakening the bound.
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

    mutating func admit(atUptimeNanoseconds now: UInt64) -> Bool {
        if let previous = lastRefillUptimeNanoseconds, now > previous {
            let elapsed = now - previous
            let (refill, refillOverflow) = elapsed.multipliedReportingOverflow(
                by: limits.sustainedTransactionsPerSecond
            )
            let boundedRefill = refillOverflow ? capacityUnits : min(refill, capacityUnits)
            let (refilled, additionOverflow) = availableUnits.addingReportingOverflow(
                boundedRefill
            )
            availableUnits = additionOverflow ? capacityUnits : min(refilled, capacityUnits)
        }
        if let previous = lastRefillUptimeNanoseconds {
            if now > previous {
                lastRefillUptimeNanoseconds = now
            }
        } else {
            lastRefillUptimeNanoseconds = now
        }

        guard availableUnits >= Self.unitsPerToken else { return false }
        availableUnits -= Self.unitsPerToken
        return true
    }
}
