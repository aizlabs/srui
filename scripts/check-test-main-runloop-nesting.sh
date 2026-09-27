#!/usr/bin/env bash
# Reject test code that nests a second CFRunLoop activation on the main run loop.
#
# Under Swift 6.2 the main run loop *is* the main-actor executor:
#
#     main -> swift_task_asyncMainDrainQueue -> CFMainExecutor.run() -> CFRunLoopRun()
#
# Any main-thread API that services or nests that run loop breaks the executor in two ways, both
# of which were measured on this suite with a DYLD interpose on `exit` and `CFRunLoopStop`:
#
#   1. The park. `-[NSButtonCell performClick:]` calls
#      `-[NSApplication nextEventMatchingMask:untilDate:inMode:dequeue:]`, which blocks in
#      `mach_msg` waiting for a window-server event that never arrives under `swift test` (and CI
#      has no window server at all). While it waits, not one main-actor job is drained, so every
#      `@MainActor` test suspended on a main-actor hop never resumes: the run sits at 0.0% CPU with
#      hundreds of tests started and none finished, and there is no failing test to point at.
#
#   2. The silent under-report. The `CFRunLoopStop(CFRunLoopGetMain())` that the concurrency
#      runtime posts to leave its drain loop is delivered by `__CFRunLoopDoBlocks` of the *nested*
#      activation, so the outer `CFRunLoopRun()` returns too. `swift_task_asyncMainDrainQueue`
#      returns, `main` returns, and the process `exit(0)`s with hundreds of tests still in flight.
#      `swift test` reports success: a green run that proved almost nothing.
#
# Target/action dispatch, and every other semantic input path this client owns, needs no event
# loop. Use `NativeActivation.click(_:)` (client-macos/Tests/SRUITests/NativeActivationTestSupport.swift).
set -uo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

# Each entry is "pattern|replacement advice".
banned=(
    'performClick(|use NativeActivation.click(_:) — dispatch target/action without an event loop'
    'nextEventMatchingMask|do not pump AppKit events from a test; drive the code path directly'
    'NSApp.run(|never start the AppKit run loop inside the test host'
    'NSApplication.shared.run(|never start the AppKit run loop inside the test host'
    '.runModal(|a modal session nests the main run loop; assert on the model instead'
    'CFRunLoopRun(|nesting the main run loop strands the main-actor executor'
    'RunLoop.main.run(|nesting the main run loop strands the main-actor executor'
    # `RunLoop.current` is `RunLoop.main` in any main-actor test, so this spelling nests the same
    # activation. It reached the suite once already, past the list above.
    'RunLoop.current.run(|nesting the main run loop strands the main-actor executor; await instead'
)

status=0
for entry in "${banned[@]}"; do
    pattern=${entry%%|*}
    advice=${entry#*|}
    # Comments and doc comments may name these APIs; only flag real call sites.
    hits=$(grep -rn --include='*.swift' -F "$pattern" client-macos/Tests 2>/dev/null |
        grep -vE '^[^:]+:[0-9]+: *(//|///|\*)' || true)
    if [ -n "$hits" ]; then
        echo "Error: main-run-loop nesting in tests ($pattern): $advice" >&2
        echo "$hits" >&2
        status=1
    fi
done

if [ "$status" -eq 0 ]; then
    echo "No main-run-loop nesting in client-macos/Tests."
fi
exit "$status"
