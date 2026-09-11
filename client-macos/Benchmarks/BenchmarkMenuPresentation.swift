import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

func benchmarkOnScreenOwnedWindowIDs() throws -> Set<CGWindowID> {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        throw BenchmarkFailure.message(
            "WindowServer did not provide the pre-menu owned-window inventory"
        )
    }

    var identifiers = Set<CGWindowID>()
    for entry in entries {
        guard let ownerPID = (
            entry[kCGWindowOwnerPID as String] as? NSNumber
        )?.int32Value else {
            throw BenchmarkFailure.message(
                "pre-menu WindowServer inventory omitted an owner PID"
            )
        }
        guard ownerPID == getpid() else { continue }
        // .optionOnScreenOnly membership is authoritative; the
        // kCGWindowIsOnscreen value itself is not guaranteed to be present.
        guard let identifier = (
            entry[kCGWindowNumber as String] as? NSNumber
        )?.uint32Value,
              identifier != 0 else {
            throw BenchmarkFailure.message(
                "pre-menu on-screen owned window omitted exact identity"
            )
        }
        identifiers.insert(identifier)
    }
    return identifiers
}

func benchmarkParseOwnedWindowEvidence(
    _ entry: [String: Any],
    expectedOwnerPID: pid_t,
    displayID: CGDirectDisplayID,
    context: String
) throws -> BenchmarkWindowServerEvidence? {
    guard let ownerPID = (
        entry[kCGWindowOwnerPID as String] as? NSNumber
    )?.int32Value else {
        throw BenchmarkFailure.message(
            "\(context) WindowServer entry omitted an owner PID"
        )
    }
    guard ownerPID == expectedOwnerPID else { return nil }
    // Callers obtained this entry with .optionOnScreenOnly. Membership in
    // that result is the on-screen proof; kCGWindowIsOnscreen is optional
    // metadata and is intentionally neither read nor required here.
    guard let identifier = (
        entry[kCGWindowNumber as String] as? NSNumber
    )?.uint32Value,
          identifier != 0,
          let layer = (
              entry[kCGWindowLayer as String] as? NSNumber
          )?.intValue,
          let alpha = (
              entry[kCGWindowAlpha as String] as? NSNumber
          )?.doubleValue,
          alpha.isFinite,
          alpha > 0,
          let boundsDictionary = entry[
              kCGWindowBounds as String
          ] as? NSDictionary,
          let bounds = CGRect(
              dictionaryRepresentation: boundsDictionary as CFDictionary
          ),
          bounds.origin.x.isFinite,
          bounds.origin.y.isFinite,
          bounds.width.isFinite,
          bounds.height.isFinite,
          bounds.width > 0,
          bounds.height > 0 else {
        throw BenchmarkFailure.message(
            "\(context) owned window omitted exact identity, layer, alpha, or geometry"
        )
    }
    return BenchmarkWindowServerEvidence(
        windowID: identifier,
        ownerPID: ownerPID,
        layer: layer,
        bounds: bounds,
        alpha: alpha,
        displayID: displayID
    )
}

func benchmarkExactOwnedMenuWindowEvidence(
    excluding baselineWindowIDs: Set<CGWindowID>,
    displayID: CGDirectDisplayID,
    menuLayer: Int
) throws -> BenchmarkWindowServerEvidence? {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        throw BenchmarkFailure.message(
            "WindowServer did not provide post-menu window evidence"
        )
    }
    let displayBounds = CGDisplayBounds(displayID)
    var candidates = [BenchmarkWindowServerEvidence]()

    for entry in entries {
        guard let evidence = try benchmarkParseOwnedWindowEvidence(
            entry,
            expectedOwnerPID: getpid(),
            displayID: displayID,
            context: "post-menu"
        ) else {
            continue
        }
        guard baselineWindowIDs.contains(evidence.windowID) == false,
              evidence.layer == menuLayer else {
            continue
        }
        guard displayBounds.contains(evidence.bounds) else {
            throw BenchmarkFailure.message(
                "new owned menu window escaped the host display"
            )
        }
        candidates.append(evidence)
    }

    guard candidates.count <= 1 else {
        throw BenchmarkFailure.message(
            "WindowServer exposed multiple new owned menu-level windows"
        )
    }
    return candidates.first
}

func benchmarkRecheckOwnedMenuWindowEvidence(
    _ expected: BenchmarkWindowServerEvidence
) throws -> BenchmarkWindowServerEvidence? {
    guard let entries = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] else {
        throw BenchmarkFailure.message(
            "WindowServer did not provide the menu recheck"
        )
    }
    let matches = entries.filter {
        ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value
            == expected.windowID
    }
    guard matches.count == 1,
          let entry = matches.first,
          let actual = try benchmarkParseOwnedWindowEvidence(
              entry,
              expectedOwnerPID: expected.ownerPID,
              displayID: expected.displayID,
              context: "menu recheck"
          ),
          actual.windowID == expected.windowID,
          actual.layer == expected.layer,
          CGDisplayBounds(expected.displayID).contains(actual.bounds) else {
        return nil
    }
    return actual
}
func benchmarkCropDisplayFrame(
    _ image: CGImage,
    to globalBounds: CGRect,
    displayBounds: CGRect
) -> CGImage? {
    guard displayBounds.contains(globalBounds),
          displayBounds.width > 0,
          displayBounds.height > 0 else {
        return nil
    }
    let scaleX = CGFloat(image.width) / displayBounds.width
    let scaleY = CGFloat(image.height) / displayBounds.height
    let pixelBounds = CGRect(
        x: (globalBounds.minX - displayBounds.minX) * scaleX,
        y: (globalBounds.minY - displayBounds.minY) * scaleY,
        width: globalBounds.width * scaleX,
        height: globalBounds.height * scaleY
    ).integral
    let imageBounds = CGRect(
        x: 0,
        y: 0,
        width: image.width,
        height: image.height
    )
    guard pixelBounds.width > 0,
          pixelBounds.height > 0,
          imageBounds.contains(pixelBounds) else {
        return nil
    }
    return image.cropping(to: pixelBounds)
}

func benchmarkAwaitOwnedMenuFrame(
    from frames: AsyncStream<BenchmarkScreenCaptureFrame>,
    baselineFrame: BenchmarkScreenCaptureFrame,
    baselineWindowIDs: Set<CGWindowID>,
    displayID: CGDirectDisplayID,
    menuLayer: Int,
    afterDisplayTime: UInt64,
    afterReceivedAt: ContinuousClock.Instant
) async throws -> BenchmarkOwnedMenuFrameProof {
    let displayBounds = CGDisplayBounds(displayID)
    let policy = FrameAcceptancePolicy()
    return try await withBenchmarkDeadline(
        "owned menu composited presentation",
        timeout: .seconds(3)
    ) {
        var lastRejection: FrameRejection?
        for await frame in frames {
            guard let candidate = try benchmarkExactOwnedMenuWindowEvidence(
                excluding: baselineWindowIDs,
                displayID: displayID,
                menuLayer: menuLayer
            ) else {
                lastRejection = .windowEvidenceUnavailable
                continue
            }
            switch policy.stableWindowEvidence(
                candidate,
                expected: candidate
            ) {
            case .success:
                break
            case let .failure(rejection):
                lastRejection = rejection
                continue
            }
            guard let baselineCrop = benchmarkCropDisplayFrame(
                baselineFrame.image,
                to: candidate.bounds,
                displayBounds: displayBounds
            ), let menuCrop = benchmarkCropDisplayFrame(
                frame.image,
                to: candidate.bounds,
                displayBounds: displayBounds
            ) else {
                lastRejection = .targetGeometryUnavailable
                continue
            }

            let acceptedPixels: FrameAcceptedPixels
            switch policy.acceptPixels(
                frame: frame,
                afterDisplayTime: afterDisplayTime,
                afterReceivedAt: afterReceivedAt,
                baselineFrame: baselineFrame,
                baselineImage: baselineCrop,
                candidateImage: menuCrop
            ) {
            case let .success(pixels):
                acceptedPixels = pixels
            case let .failure(rejection):
                lastRejection = rejection
                continue
            }

            let rechecked = try benchmarkRecheckOwnedMenuWindowEvidence(
                candidate
            )
            let acceptedWindow: BenchmarkWindowServerEvidence
            switch policy.stableWindowEvidence(
                rechecked,
                expected: candidate
            ) {
            case let .success(evidence):
                acceptedWindow = evidence
            case let .failure(rejection):
                lastRejection = rejection
                continue
            }

            return BenchmarkOwnedMenuFrameProof(
                windowEvidence: acceptedWindow,
                contentEvidence: acceptedPixels.capture.content,
                normalization: acceptedPixels.capture.normalization,
                presentedAt: frame.receivedAt,
                acceptedDisplayMachTicks: frame.displayTime
            )
        }
        throw BenchmarkFailure.message(
            "ScreenCaptureKit ended before an acceptable owned menu frame; "
                + "last rejection: "
                + (lastRejection?.description ?? "no frame received")
        )
    }
}
@MainActor
func benchmarkMeasureOwnedMenuPresentation(
    _ menu: NSMenu,
    positioningItem: NSMenuItem?,
    at location: NSPoint,
    in view: NSView,
    onActionStarting:
        (@MainActor () async throws -> Void)? = nil
) async throws -> BenchmarkMenuPresentationMeasurement {
    guard let window = view.window else {
        throw BenchmarkFailure.message(
            "menu benchmark host view is not attached to a window"
        )
    }
    let prepared = try await benchmarkPrepareExactVisibleWindow(window)
    let isolatedHostLevel = try benchmarkIsolatedHostWindowLevel()
    let menuLayer = Int(CGWindowLevelForKey(.popUpMenuWindow))
    guard window.level == isolatedHostLevel,
          prepared.evidence.layer == isolatedHostLevel.rawValue,
          prepared.evidence.layer < menuLayer else {
        throw BenchmarkFailure.message(
            "menu host was not isolated below its production pop-up level"
        )
    }
    let baselineWindowIDs = try benchmarkOnScreenOwnedWindowIDs()
    let displayID = prepared.evidence.displayID
    let displayBounds = CGDisplayBounds(displayID)
    let captureSession = try await benchmarkStartScreenCaptureStream(
        displayID: displayID,
        globalSourceRect: displayBounds,
        operation: "owned menu full-display capture"
    )

    do {
        // The most recent complete pre-action display sample is retained so the
        // exact eventual menu rectangle can be compared byte-for-byte against
        // the same rectangle before the menu existed.
        let baselineFrame = try await benchmarkAwaitFirstCompleteFrame(
            from: captureSession.frames,
            operation: "owned menu pre-action display baseline"
        )
        // Align the real transport impairment with the menu action only after
        // the full-display baseline and exact host evidence are ready.
        try await onActionStarting?()
        let actionStartedMachTicks = mach_absolute_time()
        let actionStartedAt = clock.now
        let postActionDisplayTime = actionStartedMachTicks
        let cancellationTarget = BenchmarkMenuCancellationTarget(menu: menu)
        let evidenceTask = Task.detached {
            do {
                let proof = try await benchmarkAwaitOwnedMenuFrame(
                    from: captureSession.frames,
                    baselineFrame: baselineFrame,
                    baselineWindowIDs: baselineWindowIDs,
                    displayID: displayID,
                    menuLayer: menuLayer,
                    afterDisplayTime: postActionDisplayTime,
                    afterReceivedAt: actionStartedAt
                )
                cancellationTarget.schedule()
                return proof
            } catch {
                cancellationTarget.schedule()
                throw error
            }
        }

        _ = menu.popUp(
            positioning: positioningItem,
            at: location,
            in: view
        )

        let proof: BenchmarkOwnedMenuFrameProof
        do {
            proof = try await evidenceTask.value
        } catch {
            evidenceTask.cancel()
            await captureSession.stop()
            throw error
        }
        await captureSession.stop()

        let provenance =
            "screencapturekit_same_complete_frame_new_owned_menu_window"
        let observation = OnScreenPaintObservation(
            crossedDisplayRefresh: true,
            captureAuthorization: true,
            pixelCaptureVerified: true,
            presentedAt: proof.presentedAt,
            visibilityProvenance: provenance,
            compositedContentEvidence: proof.contentEvidence,
            compositedContentNormalization: proof.normalization
        )
        benchmarkTrace(
            "menu paint provenance=\(provenance) "
                + "window=\(proof.windowEvidence.windowID) "
                + "bounds=\(proof.windowEvidence.bounds) "
                + "fingerprint="
                + String(
                    proof.contentEvidence.normalizedFingerprintSHA256
                        .prefix(16)
                )
        )
        return BenchmarkMenuPresentationMeasurement(
            actionStartedAt: actionStartedAt,
            actionStartedMachTicks: actionStartedMachTicks,
            acceptedDisplayMachTicks: proof.acceptedDisplayMachTicks,
            presentationLatencyMilliseconds:
                try benchmarkMachElapsedMilliseconds(
                    from: actionStartedMachTicks,
                    to: proof.acceptedDisplayMachTicks
                ),
            observation: observation,
            menuWindowID: proof.windowEvidence.windowID
        )
    } catch {
        await captureSession.stop()
        throw error
    }
}