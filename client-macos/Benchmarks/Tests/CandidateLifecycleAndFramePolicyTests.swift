import AppKit
import CoreGraphics
import Foundation
import Testing
@testable import BenchmarkDriver

@Test("candidate lifecycle accepts only the shared measurement order")
@MainActor
func candidateLifecycleAcceptsSharedOrder() throws {
    let descriptor = rendererCandidateMetricDescriptors[0]
    let lifecycle = RendererCandidateLifecycle(
        candidate: "test",
        metric: descriptor
    )
    for event in RendererCandidateLifecycleEvent.allCases {
        try lifecycle.record(event)
    }

    #expect(
        try lifecycle.validate()
            == "test/first_paint:warm_completed->reset_completed"
                + "->timing_started->host_attached"
                + "->representation_ingested->display_submitted"
    )
}

@Test("candidate lifecycle rejects missing and out-of-order evidence")
@MainActor
func candidateLifecycleRejectsInvalidOrder() throws {
    let descriptor = rendererCandidateMetricDescriptors[0]
    let missing = RendererCandidateLifecycle(
        candidate: "missing",
        metric: descriptor
    )
    for event in RendererCandidateLifecycleEvent.allCases
        where event != .displaySubmitted
    {
        try missing.record(event)
    }
    #expect(throws: BenchmarkFailure.self) {
        _ = try missing.validate()
    }

    let outOfOrder = RendererCandidateLifecycle(
        candidate: "out-of-order",
        metric: descriptor
    )
    for event in [
        RendererCandidateLifecycleEvent.warmCompleted,
        .resetCompleted,
        .hostAttached,
        .timingStarted,
        .representationIngested,
        .displaySubmitted,
    ] {
        try outOfOrder.record(event)
    }
    #expect(throws: BenchmarkFailure.self) {
        _ = try outOfOrder.validate()
    }
}

@Test("frame acceptance rejects stale and unchanged pixels")
func frameAcceptanceRejectsStaleAndUnchangedPixels() throws {
    let image = try policyTestImage()
    let now = ContinuousClock().now
    let baseline = BenchmarkScreenCaptureFrame(
        displayTime: 1,
        receivedAt: now,
        receivedUnixNanoseconds: 1,
        isPostArmCandidate: false,
        image: image
    )
    let stale = BenchmarkScreenCaptureFrame(
        displayTime: 10,
        receivedAt: now,
        receivedUnixNanoseconds: 2,
        isPostArmCandidate: true,
        image: image
    )
    let freshButUnchanged = BenchmarkScreenCaptureFrame(
        displayTime: 11,
        receivedAt: now,
        receivedUnixNanoseconds: 3,
        isPostArmCandidate: true,
        image: image
    )
    let policy = FrameAcceptancePolicy()

    switch policy.acceptPixels(
        frame: stale,
        afterDisplayTime: 10,
        afterReceivedAt: now,
        baselineFrame: baseline,
        baselineImage: image,
        candidateImage: image
    ) {
    case .failure(.beforeDisplayCutoff):
        break
    default:
        Issue.record("stale frame crossed the display cutoff")
    }

    switch policy.acceptPixels(
        frame: freshButUnchanged,
        afterDisplayTime: 10,
        afterReceivedAt: now,
        baselineFrame: baseline,
        baselineImage: image,
        candidateImage: image
    ) {
    case .failure(.insufficientMaterialChange):
        break
    default:
        Issue.record("unchanged frame passed the material-delta predicate")
    }
}

@Test("passive frame boundary optionally accepts frames during action")
func passiveFrameBoundarySelection() {
    let actionStartedAt = ContinuousClock().now
    let actionCompletedAt = actionStartedAt + .milliseconds(8)

    let defaultBoundary = benchmarkPassiveFrameAcceptanceBoundary(
        acceptFramesDuringAction: false,
        actionStartedMachTicks: 100,
        actionStartedAt: actionStartedAt,
        actionCompletedMachTicks: 200,
        actionCompletedAt: actionCompletedAt
    )
    #expect(defaultBoundary.displayTime == 200)
    #expect(defaultBoundary.receivedAt == actionCompletedAt)

    let duringActionBoundary = benchmarkPassiveFrameAcceptanceBoundary(
        acceptFramesDuringAction: true,
        actionStartedMachTicks: 100,
        actionStartedAt: actionStartedAt,
        actionCompletedMachTicks: 200,
        actionCompletedAt: actionCompletedAt
    )
    #expect(duringActionBoundary.displayTime == 100)
    #expect(duringActionBoundary.receivedAt == actionStartedAt)
}

@Test("window acceptance rejects foreign and intersecting evidence")
func windowAcceptanceRejectsForeignAndOverlap() {
    let expected = policyTestWindow(windowID: 7, ownerPID: 41)
    let foreign = policyTestWindow(windowID: 8, ownerPID: 99)
    let policy = FrameAcceptancePolicy()

    switch policy.acceptWindowEvidence(
        foreign,
        expected: expected,
        intersectingWindow: nil
    ) {
    case .failure(.windowEvidenceChanged):
        break
    default:
        Issue.record("foreign WindowServer identity was accepted")
    }

    switch policy.acceptWindowEvidence(
        expected,
        expected: expected,
        intersectingWindow: "window=9 owner=foreign"
    ) {
    case .failure(.intersectedWindow):
        break
    default:
        Issue.record("intersecting WindowServer evidence was accepted")
    }
}

@Test("local interaction pointer targets follow window relocation")
@MainActor
func localInteractionPointerTargetsFollowWindowRelocation() throws {
    let window = NSWindow(
        contentRect: NSRect(x: 100, y: 100, width: 300, height: 200),
        styleMask: .borderless,
        backing: .buffered,
        defer: false
    )
    let button = NSButton(
        frame: NSRect(x: 20, y: 30, width: 100, height: 40)
    )
    let contentView = try #require(window.contentView)
    contentView.addSubview(button)

    let beforeMove = benchmarkScreenCenter(of: button, in: window)
    window.setFrameOrigin(NSPoint(x: 300, y: 400))
    let afterMove = benchmarkScreenCenter(of: button, in: window)

    #expect(abs((afterMove.x - beforeMove.x) - 200) < 0.001)
    #expect(abs((afterMove.y - beforeMove.y) - 300) < 0.001)
    let outside = benchmarkScreenPointOutside(button, in: window)
    let outsideInWindow = window.convertPoint(fromScreen: outside)
    #expect(
        button.bounds.contains(button.convert(outsideInWindow, from: nil))
            == false
    )
}

@Test("console lock-state parser distinguishes locked, unlocked, and absent")
func consoleLockStateParser() {
    #expect(
        benchmarkSessionLockState(
            from: ["CGSSessionScreenIsLocked": NSNumber(value: 1)]
        ) == true
    )
    #expect(
        benchmarkSessionLockState(
            from: ["CGSSessionScreenIsLocked": NSNumber(value: 0)]
        ) == false
    )
    #expect(benchmarkSessionLockState(from: [:]) == nil)
}

@Test("on-screen menu evidence accepts omission of the optional flag")
func menuEvidenceAcceptsOmittedOnscreenFlag() throws {
    let bounds = CGRect(x: 10, y: 20, width: 120, height: 80)
    let entry: [String: Any] = [
        kCGWindowOwnerPID as String: NSNumber(value: 41),
        kCGWindowNumber as String: NSNumber(value: 77),
        kCGWindowLayer as String: NSNumber(value: 101),
        kCGWindowAlpha as String: NSNumber(value: 1.0),
        kCGWindowBounds as String:
            bounds.dictionaryRepresentation as NSDictionary,
    ]
    #expect(entry[kCGWindowIsOnscreen as String] == nil)

    let parsed = try benchmarkParseOwnedWindowEvidence(
        entry,
        expectedOwnerPID: 41,
        displayID: 1,
        context: "test"
    )
    let evidence = try #require(parsed)
    #expect(evidence.windowID == 77)
    #expect(evidence.ownerPID == 41)
    #expect(evidence.layer == 101)
    #expect(evidence.alpha == 1)
    #expect(evidence.bounds == bounds)
}

private func policyTestWindow(
    windowID: CGWindowID,
    ownerPID: pid_t
) -> BenchmarkWindowServerEvidence {
    BenchmarkWindowServerEvidence(
        windowID: windowID,
        ownerPID: ownerPID,
        layer: 20,
        bounds: CGRect(x: 10, y: 10, width: 16, height: 16),
        alpha: 1,
        displayID: 1
    )
}

private func policyTestImage() throws -> CGImage {
    let width = 16
    let height = 16
    guard let context = CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: width * 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        throw BenchmarkFailure.message("could not create policy test context")
    }
    for y in 0..<height {
        for x in 0..<width {
            let light = (x + y).isMultiple(of: 2)
            context.setFillColor(
                red: light ? 0.9 : 0.1,
                green: light ? 0.7 : 0.2,
                blue: light ? 0.2 : 0.8,
                alpha: 1
            )
            context.fill(CGRect(x: x, y: y, width: 1, height: 1))
        }
    }
    guard let image = context.makeImage() else {
        throw BenchmarkFailure.message("could not create policy test image")
    }
    return image
}
