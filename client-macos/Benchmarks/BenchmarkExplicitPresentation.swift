import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

struct BenchmarkExplicitPaintTarget {
    let window: NSWindow
    let targetView: NSView
}

@MainActor
func benchmarkMeasureExplicitCompositedPaint(
    on screen: NSScreen,
    onActionStarted:
        (@MainActor (ContinuousClock.Instant, UInt64) throws -> Void)? = nil,
    onDisplaySubmitted: (@MainActor () throws -> Void)? = nil,
    requiredContentChangeFromObservation:
        OnScreenPaintObservation? = nil,
    action:
        @escaping @MainActor () async throws -> BenchmarkExplicitPaintTarget,
    onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)? = nil
) async throws -> BenchmarkPassivePresentationMeasurement {
    let requiredContentChange: (
        content: BenchmarkCompositedContentEvidence,
        normalization: BenchmarkCompositedContentNormalization
    )?
    if let requiredContentChangeFromObservation {
        guard requiredContentChangeFromObservation.captureAuthorization,
              requiredContentChangeFromObservation.pixelCaptureVerified,
              let content =
                requiredContentChangeFromObservation.compositedContentEvidence,
              let normalization =
                requiredContentChangeFromObservation
                    .compositedContentNormalization else {
            throw BenchmarkFailure.message(
                "required prior paint state lacked authorized normalized "
                    + "composited pixel evidence"
            )
        }
        requiredContentChange = (content, normalization)
    } else {
        requiredContentChange = nil
    }
    guard let screenNumber = screen.deviceDescription[
        NSDeviceDescriptionKey("NSScreenNumber")
    ] as? NSNumber else {
        throw BenchmarkFailure.message(
            "explicit paint screen has no CoreGraphics display identity"
        )
    }
    let displayID = CGDirectDisplayID(screenNumber.uint32Value)
    let displayBounds = CGDisplayBounds(displayID)
    let presentationBoundary = BenchmarkScreenCaptureBoundary(
        onPresented: onPresented
    )
    let captureSession = try await benchmarkStartScreenCaptureStream(
        displayID: displayID,
        globalSourceRect: displayBounds,
        operation: "explicit full-display paint capture",
        presentationBoundary: presentationBoundary
    )

    do {
        let baselineFrame = try await benchmarkAwaitFirstCompleteFrame(
            from: captureSession.frames,
            operation: "explicit pre-action display baseline"
        )
        let preActionPointerLocation =
            try benchmarkParkPointerOutsideMeasurementROI(
                on: screen,
                side: .left
            )
        defer {
            benchmarkRestorePointer(preActionPointerLocation)
        }
        let actionStartedMachTicks = mach_absolute_time()
        let actionStartedAt = clock.now
        try onActionStarted?(actionStartedAt, actionStartedMachTicks)
        let target = try await action()
        let actionCompletedAt = clock.now

        let targetScreen = target.window.screen ?? screen
        guard target.window.isVisible == false,
              target.targetView.window === target.window,
              let targetScreenNumber = targetScreen.deviceDescription[
                  NSDeviceDescriptionKey("NSScreenNumber")
              ] as? NSNumber,
              CGDirectDisplayID(targetScreenNumber.uint32Value) == displayID else {
            throw BenchmarkFailure.message(
                "explicit paint action must return a hidden target attached to "
                    + "the prepared capture display"
            )
        }

        let application = NSApplication.shared
        // The benchmark-only host joins ordinary Spaces and eligible Stage Manager/full-screen
        // application sets before ordering. Exact WindowServer visibility, z-order, and captured
        // pixels remain fail-closed below.
        target.window.collectionBehavior.formUnion([
            .canJoinAllSpaces,
            .canJoinAllApplications,
        ])
        if application.activationPolicy() == .regular {
            application.activate()
        }
        let usesAppKitVisiblePath = application.isActive
        let isolatedHostLevel = try benchmarkIsolatedHostWindowLevel()
        target.window.animationBehavior = .none
        target.window.level = isolatedHostLevel
        target.window.contentView?.layoutSubtreeIfNeeded()

        let submissionDisplayTime = mach_absolute_time()
        presentationBoundary.arm(afterDisplayTime: submissionDisplayTime)
        target.window.makeKeyAndOrderFront(nil)
        target.window.orderFrontRegardless()
        CATransaction.flush()
        try onDisplaySubmitted?()

        typealias AcceptedCandidate = (
            frame: BenchmarkScreenCaptureFrame,
            evidence: FrameTargetEvidence,
            pixels: FrameAcceptedPixels
        )
        let policy = FrameAcceptancePolicy()
        var acceptedCandidate: AcceptedCandidate?
        var rejectedCandidateCount = 0
        var lastRejection: FrameRejection?

        let presentationTimeoutTask = Task {
            do {
                try await Task.sleep(for: .seconds(10))
            } catch {
                return
            }
            captureSession.finishFrames()
        }
        defer {
            presentationTimeoutTask.cancel()
        }

        candidateLoop:
        for await candidateFrame in captureSession.frames
            where candidateFrame.isPostArmCandidate
        {
            rejectedCandidateCount += 1
            let targetEvidence: FrameTargetEvidence
            switch policy.targetEvidence(
                window: target.window,
                targetView: target.targetView,
                screen: screen,
                requiredLayer: isolatedHostLevel.rawValue,
                requiredDisplayID: displayID,
                requireActiveAppKitVisibility: usesAppKitVisiblePath
            ) {
            case let .success(evidence):
                targetEvidence = evidence
            case let .failure(rejection):
                lastRejection = rejection
                continue candidateLoop
            }

            guard let baselineCrop = benchmarkCropDisplayFrame(
                baselineFrame.image,
                to: targetEvidence.targetBounds,
                displayBounds: displayBounds
            ), let candidateCrop = benchmarkCropDisplayFrame(
                candidateFrame.image,
                to: targetEvidence.targetBounds,
                displayBounds: displayBounds
            ) else {
                lastRejection = .targetGeometryUnavailable
                continue candidateLoop
            }

            let acceptedPixels: FrameAcceptedPixels
            switch policy.acceptPixels(
                frame: candidateFrame,
                afterDisplayTime: submissionDisplayTime,
                afterReceivedAt: actionCompletedAt,
                baselineFrame: baselineFrame,
                baselineImage: baselineCrop,
                candidateImage: candidateCrop,
                requiredPriorContent: requiredContentChange?.content,
                requiredPriorNormalization:
                    requiredContentChange?.normalization
            ) {
            case let .success(pixels):
                acceptedPixels = pixels
            case let .failure(rejection):
                lastRejection = rejection
                continue candidateLoop
            }

            switch policy.targetEvidence(
                window: target.window,
                targetView: target.targetView,
                screen: screen,
                requiredLayer: isolatedHostLevel.rawValue,
                requiredDisplayID: displayID,
                expected: targetEvidence,
                requireActiveAppKitVisibility: usesAppKitVisiblePath
            ) {
            case .success:
                break
            case let .failure(rejection):
                lastRejection = rejection
                continue candidateLoop
            }
            guard presentationBoundary.accept(
                displayTime: candidateFrame.displayTime,
                receivedAt: candidateFrame.receivedAt,
                receivedUnixNanoseconds:
                    candidateFrame.receivedUnixNanoseconds
            ) else {
                lastRejection = .captureBoundaryRejected
                continue candidateLoop
            }

            acceptedCandidate = (
                candidateFrame,
                targetEvidence,
                acceptedPixels
            )
            break candidateLoop
        }
        guard let acceptedCandidate else {
            throw BenchmarkFailure.message(
                "explicit composited presentation timed out or ended after "
                    + "\(rejectedCandidateCount) post-cutoff candidate(s); "
                    + "last rejection: "
                    + (lastRejection?.description ?? "no frame received")
                    + "; final target state: window_number="
                    + "\(target.window.windowNumber) visible=\(target.window.isVisible) "
                    + "miniaturized=\(target.window.isMiniaturized) "
                    + "active_space=\(target.window.isOnActiveSpace) "
                    + "collection_behavior=\(target.window.collectionBehavior.rawValue) "
                    + "activation_policy=\(application.activationPolicy().rawValue) "
                    + "app_active=\(application.isActive) "
                    + "app_hidden=\(application.isHidden) "
                    + benchmarkWindowServerEntryDiagnostic(for: target.window)
            )
        }
        let visibilityProvenance = usesAppKitVisiblePath
            ? "screencapturekit_first_complete_target_frame_status_level_"
                + "\(isolatedHostLevel.rawValue)_appkit_active"
            : "screencapturekit_first_complete_target_frame_status_level_"
                + "\(isolatedHostLevel.rawValue)_appkit_inactive"
        let observation = OnScreenPaintObservation(
            crossedDisplayRefresh: true,
            captureAuthorization: true,
            pixelCaptureVerified: true,
            presentedAt: acceptedCandidate.frame.receivedAt,
            visibilityProvenance: visibilityProvenance,
            compositedContentEvidence:
                acceptedCandidate.pixels.capture.content,
            compositedContentNormalization:
                acceptedCandidate.pixels.capture.normalization
        )
        await captureSession.stop()
        benchmarkTrace(
            "explicit paint provenance=\(visibilityProvenance) "
                + "window=\(acceptedCandidate.evidence.window.windowID) "
                + "target=\(acceptedCandidate.evidence.targetBounds) "
                + "fingerprint="
                + String(
                    acceptedCandidate.pixels.capture.content
                        .normalizedFingerprintSHA256.prefix(16)
                )
        )
        return BenchmarkPassivePresentationMeasurement(
            actionStartedAt: actionStartedAt,
            actionCompletedAt: actionCompletedAt,
            actionStartedMachTicks: actionStartedMachTicks,
            acceptedDisplayMachTicks:
                acceptedCandidate.frame.displayTime,
            presentationLatencyMilliseconds:
                try benchmarkMachElapsedMilliseconds(
                    from: actionStartedMachTicks,
                    to: acceptedCandidate.frame.displayTime
                ),
            observation: observation
        )
    } catch {
        await captureSession.stop()
        throw error
    }
}
@MainActor
func benchmarkPrepareExactVisibleWindow(
    _ window: NSWindow
) async throws -> (
    screen: NSScreen,
    visibilityProvenance: String,
    evidence: BenchmarkWindowServerEvidence,
    clientContentBounds: CGRect
) {
    guard CGPreflightScreenCaptureAccess(),
          let screen = window.screen ?? NSScreen.main,
          window.contentView != nil else {
        throw BenchmarkFailure.message(
            "composited preparation requires capture authorization, a display, "
                + "and a client content view"
        )
    }

    let availableFrame = screen.visibleFrame
    let windowFrame = window.frame
    guard windowFrame.width <= availableFrame.width,
          windowFrame.height <= availableFrame.height else {
        throw BenchmarkFailure.message(
            "benchmark host window does not fit on the selected display: "
                + "window=\(windowFrame), available=\(availableFrame)"
        )
    }
    // AppKit cascades newly created windows across launches. Repeated focused
    // runs can otherwise leave a later process partially offscreen. Use a
    // deterministic untimed left-side position instead of screen center, where
    // macOS commonly presents transient system progress surfaces. This changes
    // no z-order rule: any surface that intersects the target still fails closed.
    let horizontalInset = min(16, max(0, availableFrame.width - windowFrame.width))
    window.setFrameOrigin(
        NSPoint(
            x: floor(availableFrame.minX + horizontalInset),
            y: floor(availableFrame.midY - windowFrame.height / 2)
        )
    )
    // This benchmark-only host joins ordinary Spaces and eligible Stage Manager/full-screen
    // application sets. Exact visibility, z-order, geometry, and pixel acceptance remain
    window.collectionBehavior.formUnion([
        .canJoinAllSpaces,
        .canJoinAllApplications,
    ])
    let isolatedHostLevel = try benchmarkIsolatedHostWindowLevel()
    window.animationBehavior = .none
    window.level = isolatedHostLevel
    func submitUntimedWindow() {
        if NSApplication.shared.activationPolicy() == .regular {
            NSApplication.shared.activate()
        }
        window.makeKeyAndOrderFront(nil)
        window.orderFrontRegardless()
        window.contentView?.layoutSubtreeIfNeeded()
        window.contentView?.needsDisplay = true
        window.displayIfNeeded()
        CATransaction.flush()
    }

    func awaitExactEvidence() async throws -> BenchmarkWindowServerEvidence {
        // Pointer relocation can start an untimed Dock/menu-bar retraction.
        // Give WindowServer the same bounded settling window used by the
        // subsequent clear-z-order proof; the timed action has not begun.
        let deadline = Date().addingTimeInterval(10)
        var evidence = exactWindowServerEvidence(for: window, on: screen)
        while (window.isVisible == false || evidence == nil), Date() < deadline {
            submitUntimedWindow()
            try await Task.sleep(for: .milliseconds(1))
            evidence = exactWindowServerEvidence(for: window, on: screen)
        }
        guard window.isVisible, let evidence else {
            throw BenchmarkFailure.message(
                "untimed preparation did not acquire exact WindowServer "
                    + "identity, on-screen state, layer, bounds, and display: "
                    + "window_number=\(window.windowNumber) "
                    + "window_frame=\(window.frame) screen_frame=\(screen.frame) "
                    + "visible_frame=\(screen.visibleFrame) "
                    + "level=\(window.level.rawValue) is_visible=\(window.isVisible) "
                    + benchmarkWindowServerEntryDiagnostic(for: window)
            )
        }
        return evidence
    }

    submitUntimedWindow()
    var evidence = try await awaitExactEvidence()
    guard evidence.layer == isolatedHostLevel.rawValue else {
        throw BenchmarkFailure.message(
            "benchmark host did not reach isolated WindowServer level "
                + "\(isolatedHostLevel.rawValue)"
        )
    }
    // NSWindow.occlusionState is advisory and can remain empty for a
    // command-line AppKit process even when WindowServer reports the exact
    // on-screen window. Record it, but prove visibility below with exact
    // z-order plus ScreenCaptureKit pixels from the target ROI.
    let appKitOcclusionRawValue = window.occlusionState.rawValue
    let visibilityProvenance =
        "screencapturekit_composited_baseline_status_level_"
            + "\(isolatedHostLevel.rawValue)_appkit_occlusion_"
            + "\(appKitOcclusionRawValue)"

    // WindowServer can publish opening-animation geometry before the exact
    // final client rectangle. Require the same exact identity and geometry
    // across 300 ms—longer than an ordinary AppKit opening transition—before
    // starting the baseline stream. The timed action begins only afterwards.
    let stableGeometryDeadline = Date().addingTimeInterval(10)
    var clientContentBounds: CGRect?
    while clientContentBounds == nil, Date() < stableGeometryDeadline {
        guard let candidateEvidence = exactWindowServerEvidence(
            for: window,
            on: screen
        ),
              let candidateClientContentBounds = exactClientContentBounds(
                  for: window,
                  on: screen,
                  evidence: candidateEvidence
              ) else {
            try await Task.sleep(for: .milliseconds(1))
            continue
        }
        try await Task.sleep(for: .milliseconds(300))
        guard let recheckedEvidence = exactWindowServerEvidence(
            for: window,
            on: screen
        ),
              let recheckedClientContentBounds = exactClientContentBounds(
                  for: window,
                  on: screen,
                  evidence: recheckedEvidence
              ),
              recheckedEvidence == candidateEvidence,
              recheckedClientContentBounds
                  == candidateClientContentBounds else {
            continue
        }
        evidence = recheckedEvidence
        clientContentBounds = recheckedClientContentBounds
    }
    guard let clientContentBounds else {
        throw BenchmarkFailure.message(
            "prepared window never reached exact final client geometry within "
                + "the untimed 10-second settling deadline: "
                + "window_frame=\(window.frame) "
                + benchmarkWindowServerEntryDiagnostic(for: window)
        )
    }
    // Transient system progress or shielding surfaces can legitimately
    // appear while preparation is still untimed. Wait for a genuinely clear
    // z-order instead of whitelisting an owner or accepting contaminated
    // pixels. Once timing starts, every equivalent intersection still fails.
    let unobscuredDeadline = Date().addingTimeInterval(10)
    var lastIntersection: String?
    var unobscuredEvidence: BenchmarkWindowServerEvidence?
    while Date() < unobscuredDeadline {
        guard let candidateEvidence = exactWindowServerEvidence(
            for: window,
            on: screen
        ),
              candidateEvidence == evidence else {
            try await Task.sleep(for: .milliseconds(10))
            continue
        }
        if let intersection =
            windowServerNonzeroAlphaIntersectionAbove(candidateEvidence)
        {
            lastIntersection = intersection
            try await Task.sleep(for: .milliseconds(10))
            continue
        }
        unobscuredEvidence = candidateEvidence
        break
    }
    guard let unobscuredEvidence else {
        throw BenchmarkFailure.message(
            "prepared window did not obtain an unobscured z-order before the "
                + "untimed deadline; last intersection: "
                + (lastIntersection ?? "WindowServer evidence unavailable")
        )
    }
    return (
        screen,
        visibilityProvenance,
        unobscuredEvidence,
        clientContentBounds
    )
}
