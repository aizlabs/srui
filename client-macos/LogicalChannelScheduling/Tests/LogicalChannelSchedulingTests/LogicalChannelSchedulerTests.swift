//
// LogicalChannelSchedulerTests.swift
// LogicalChannelSchedulingTests
//
// Algorithmic coverage for the portable scheduler on Linux and macOS (§18.2, §19.2).
//
// SocketWriter FIFO, blocked-write, and backpressure tests stay in the macOS
// SRUITests suite: they exercise platform transport, not this module.
//

import Testing
@testable import LogicalChannelScheduling

@Suite("Logical Channel Scheduler (§19.2)")
struct LogicalChannelSchedulerTests {

    @Test("Service cycle is the documented 24-slot sequence")
    func serviceCycleMatchesDocumentedSequence() {
        #expect(LogicalChannelScheduler.serviceCycle == [
            .control, .input, .ui,
            .control, .input, .terminalHigh,
            .control, .input, .ui,
            .control, .input, .terminalNormal,
            .control, .input, .ui,
            .control, .input, .terminalHigh,
            .ui, .control, .input,
            .terminalNormal, .terminalHigh, .resource,
        ])
        #expect(LogicalChannelScheduler.serviceCycle.count == 24)
    }

    @Test("Each class declares its saturation service gap")
    func declaredMaximumServiceGaps() {
        #expect(LogicalChannelClass.control.maxServiceGap == 5)
        #expect(LogicalChannelClass.input.maxServiceGap == 5)
        #expect(LogicalChannelClass.ui.maxServiceGap == 8)
        #expect(LogicalChannelClass.terminalHigh.maxServiceGap == 12)
        #expect(LogicalChannelClass.terminalNormal.maxServiceGap == 14)
        #expect(LogicalChannelClass.resource.maxServiceGap == 24)
    }

    @Test("Saturated cycle preserves per-lane FIFO and documented service gaps")
    func saturatedCyclePreservesFIFOAndBounds() throws {
        var queues: [LogicalChannelClass: [UInt32]] = Dictionary(
            uniqueKeysWithValues: LogicalChannelClass.allCases.map { ($0, []) }
        )
        for logicalClass in LogicalChannelClass.allCases {
            queues[logicalClass] = Array(0..<12)
        }

        var scheduler = LogicalChannelScheduler()
        var lastIndex: [LogicalChannelClass: Int] = [:]
        var nextExpected: [LogicalChannelClass: UInt32] = Dictionary(
            uniqueKeysWithValues: LogicalChannelClass.allCases.map { ($0, 0) }
        )
        var observedMaxGap: [LogicalChannelClass: Int] = [:]

        for index in 0..<(LogicalChannelScheduler.serviceCycle.count * 6) {
            for logicalClass in LogicalChannelClass.allCases {
                if queues[logicalClass]?.isEmpty == true {
                    queues[logicalClass, default: []].append(nextExpected[logicalClass] ?? 0)
                }
            }
            let selected = scheduler.selectNext { candidate in
                !(queues[candidate] ?? []).isEmpty
            }
            let logicalClass = try #require(selected)
            let token = queues[logicalClass]!.removeFirst()
            #expect(token == nextExpected[logicalClass])
            nextExpected[logicalClass, default: 0] += 1
            if let previous = lastIndex[logicalClass] {
                let gap = index - previous
                #expect(gap <= logicalClass.maxServiceGap)
                observedMaxGap[logicalClass] = max(observedMaxGap[logicalClass] ?? 0, gap)
            }
            lastIndex[logicalClass] = index
        }

        for logicalClass in LogicalChannelClass.allCases {
            let observed = try #require(observedMaxGap[logicalClass])
            #expect(observed == logicalClass.maxServiceGap)
        }
    }

    @Test("Empty lanes are skipped without consuming a dispatch")
    func emptyLanesAreSkippedWithoutConsumingADispatch() {
        var resource = [UInt32]([1, 2])
        var scheduler = LogicalChannelScheduler()
        var dispatches = 0

        let first = scheduler.selectNext { candidate in
            candidate == .resource && !resource.isEmpty
        }
        #expect(first == .resource)
        dispatches += 1
        #expect(resource.removeFirst() == 1)

        let second = scheduler.selectNext { candidate in
            candidate == .resource && !resource.isEmpty
        }
        #expect(second == .resource)
        dispatches += 1
        #expect(resource.removeFirst() == 2)

        #expect(scheduler.selectNext { candidate in
            candidate == .resource && !resource.isEmpty
        } == nil)
        #expect(dispatches == 2)
        #expect(resource.isEmpty)
    }

    @Test("Cursor progresses across successive calls instead of restarting the cycle")
    func cursorProgressesAcrossSuccessiveCalls() {
        var scheduler = LogicalChannelScheduler()
        var inspected: [LogicalChannelClass] = []

        let first = scheduler.selectNext { candidate in
            inspected.append(candidate)
            return candidate == .ui
        }
        #expect(first == .ui)
        #expect(inspected == [.control, .input, .ui])

        inspected = []
        let second = scheduler.selectNext { candidate in
            inspected.append(candidate)
            return candidate == .ui
        }
        #expect(second == .ui)
        #expect(inspected == [.control, .input, .terminalHigh, .control, .input, .ui])
    }

    @Test("A single ready lane is selected repeatedly")
    func singleReadyLaneIsSelectedRepeatedly() {
        var scheduler = LogicalChannelScheduler()
        var selected: [LogicalChannelClass] = []
        for _ in 0..<8 {
            selected.append(scheduler.selectNext { $0 == .input }!)
        }
        #expect(selected.allSatisfy { $0 == .input })
        #expect(selected.count == 8)
    }

    @Test("All lanes empty returns nil and inspects every slot once")
    func allLanesEmptyReturnsNil() {
        var scheduler = LogicalChannelScheduler()
        var inspected = 0
        let selected = scheduler.selectNext { _ in
            inspected += 1
            return false
        }
        #expect(selected == nil)
        #expect(inspected == LogicalChannelScheduler.serviceCycle.count)
    }

    @Test("Selection is deterministic across cycle wraparound")
    func selectionIsDeterministicAcrossWraparound() {
        var scheduler = LogicalChannelScheduler()
        var firstCycle: [LogicalChannelClass] = []
        for _ in 0..<LogicalChannelScheduler.serviceCycle.count {
            firstCycle.append(scheduler.selectNext { _ in true }!)
        }
        #expect(firstCycle == LogicalChannelScheduler.serviceCycle)

        var secondCycle: [LogicalChannelClass] = []
        for _ in 0..<LogicalChannelScheduler.serviceCycle.count {
            secondCycle.append(scheduler.selectNext { _ in true }!)
        }
        #expect(secondCycle == LogicalChannelScheduler.serviceCycle)
        #expect(scheduler.selectNext { _ in true } == .control)
    }
}
