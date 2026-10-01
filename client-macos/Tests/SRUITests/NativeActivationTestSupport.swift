//
// NativeActivationTestSupport.swift
// SRUITests
//
// Event-loop-free native control activation for tests (§7.6, §7.7, §22.9, §32.11).
//
// `NSControl.performClick(_:)` must never be used from this suite. It routes through
// `NSButtonCell`, which calls `-[NSApplication nextEventMatchingMask:untilDate:inMode:dequeue:]`
// to hold the control highlighted for one event cycle. That **nests a second CFRunLoop
// activation on the main run loop** — and under Swift 6.2 the main run loop *is* the main-actor
// executor:
//
//     main -> swift_task_asyncMainDrainQueue -> CFMainExecutor.run() -> CFRunLoopRun()
//
// Nesting inside that activation produced the two failures that made this suite unusable, both
// measured with a `DYLD_INSERT_LIBRARIES` interpose on `exit`/`CFRunLoopStop`:
//
//  1. The park. The nested activation blocks in `mach_msg` inside `__CFRunLoopServiceMachPort`
//     waiting for a window-server event that never arrives (CI has no window server, and a
//     synthetic click has no event to dequeue). While it waits, the main-actor executor cannot
//     drain a single job, so every `@MainActor` test suspended on a main-actor hop never resumes:
//     the whole run sits at 0.0% CPU with hundreds of tests started and none completed.
//
//  2. The silent under-report. The `CFRunLoopStop(CFRunLoopGetMain())` that the concurrency
//     runtime posts to leave its drain loop is delivered by `__CFRunLoopDoBlocks` of the *nested*
//     activation, so the outer `CFRunLoopRun()` returns too. `swift_task_asyncMainDrainQueue`
//     returns, `main` returns, and the process calls `exit(0)` with hundreds of tests still in
//     flight — a "green" run that proved almost nothing.
//
// Target/action dispatch needs no event loop at all, so tests drive it directly.
// `scripts/check-test-event-loop-nesting.sh` keeps `performClick(_:)` (and the other main-run-loop
// nesting calls) out of the suite so this cannot be reintroduced.
//

import AppKit
import Testing

@MainActor
enum NativeActivation {

    /// Dispatches `control`'s target/action exactly as a real click does, without an event loop.
    ///
    /// Besides dispatch, this mirrors the one click semantic the tests depend on: a disabled
    /// control does not send its action. Controls whose click also mutates `state` (checkbox,
    /// switch, radio) are *not* covered — drive those through their own adapter instead, because
    /// the state transition, not the dispatch, is what those tests assert.
    static func click(
        _ control: NSControl,
        sourceLocation: SourceLocation = #_sourceLocation
    ) {
        guard control.isEnabled else { return }
        guard let action = control.action else {
            Issue.record(
                "control \(type(of: control)) has no action to dispatch",
                sourceLocation: sourceLocation
            )
            return
        }
        if !control.sendAction(action, to: control.target) {
            Issue.record(
                "target/action dispatch of \(action) was not delivered",
                sourceLocation: sourceLocation
            )
        }
    }
}
