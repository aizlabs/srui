import AppKit
import CoreMedia
import CoreVideo
import Darwin
import Foundation
import QuartzCore
import ScreenCaptureKit
import VideoToolbox
import WebKit

func normalizedCompositedContentEvidence(
    _ image: CGImage,
    markerColors: [BenchmarkVisibilityMarkerColor]? = nil,
    normalization suppliedNormalization:
        BenchmarkCompositedContentNormalization? = nil
) -> BenchmarkCompositedCaptureEvidence? {
    guard image.width > 0, image.height > 0 else { return nil }
    let bytesPerRow = image.width * 4
    let (byteCount, overflow) = bytesPerRow.multipliedReportingOverflow(
        by: image.height
    )
    guard overflow == false, byteCount > 0 else { return nil }

    let pixels = UnsafeMutablePointer<UInt8>.allocate(capacity: byteCount)
    pixels.initialize(repeating: 0, count: byteCount)
    defer {
        pixels.deinitialize(count: byteCount)
        pixels.deallocate()
    }
    guard let context = CGContext(
        data: pixels,
        width: image.width,
        height: image.height,
        bitsPerComponent: 8,
        bytesPerRow: bytesPerRow,
        space: CGColorSpace(name: CGColorSpace.sRGB)
            ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue
            | CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        return nil
    }
    context.interpolationQuality = .none
    context.draw(
        image,
        in: CGRect(x: 0, y: 0, width: image.width, height: image.height)
    )

    let resolvedNormalization: BenchmarkCompositedContentNormalization
    let markerVisible: Bool
    if let suppliedNormalization {
        guard suppliedNormalization.pixelWidth == image.width,
              suppliedNormalization.pixelHeight == image.height else {
            return nil
        }
        resolvedNormalization = suppliedNormalization
        markerVisible = false
    } else {
        guard let markerColors, markerColors.count >= 3 else { return nil }
        let tolerance = 32
        func matches(
            x: Int,
            y: Int,
            color: BenchmarkVisibilityMarkerColor
        ) -> Bool {
            let offset = y * bytesPerRow + x * 4
            return abs(Int(pixels[offset]) - Int(color.red)) <= tolerance
                && abs(Int(pixels[offset + 1]) - Int(color.green)) <= tolerance
                && abs(Int(pixels[offset + 2]) - Int(color.blue)) <= tolerance
                && pixels[offset + 3] >= 224
        }

        var markerMatch: BenchmarkVisibilityMarkerMatch?
        markerSearch: for y in 0..<image.height {
            for x in 0..<image.width
            where matches(x: x, y: y, color: markerColors[0]) {
                let remaining = image.width - 1 - x
                let maximumStep = min(
                    64,
                    remaining / (markerColors.count - 1)
                )
                guard maximumStep >= 2 else { continue }
                for step in 2...maximumStep {
                    var complete = true
                    for index in 1..<markerColors.count where complete {
                        complete = matches(
                            x: x + index * step,
                            y: y,
                            color: markerColors[index]
                        )
                    }
                    if complete {
                        markerMatch = BenchmarkVisibilityMarkerMatch(
                            x: x,
                            y: y,
                            step: step
                        )
                        break markerSearch
                    }
                }
            }
        }
        guard let markerMatch else { return nil }

        // Mask the complete marker and a generous anti-aliasing fringe. The
        // passive observer reuses these exact coordinates, so removing the
        // preparation probe cannot manufacture a content change.
        resolvedNormalization = BenchmarkCompositedContentNormalization(
            pixelWidth: image.width,
            pixelHeight: image.height,
            markerMaskMinX: 0,
            markerMaskMaxX: min(
                image.width,
                max(
                    markerMatch.x
                        + (markerColors.count + 2) * markerMatch.step,
                    8 * markerMatch.step
                )
            ),
            markerMaskMinY: max(
                0,
                markerMatch.y - 4 * markerMatch.step
            ),
            markerMaskMaxY: min(
                image.height,
                markerMatch.y + 4 * markerMatch.step
            )
        )
        markerVisible = true
    }

    // The crop itself is client content only; window chrome, shadows, and
    // wallpaper never enter this normalized RGBA8 byte stream.
    var normalizedBytes = [UInt8]()
    let header = "\(image.width)x\(image.height):rgba8-srgb\n".utf8
    normalizedBytes.reserveCapacity(header.count + byteCount)
    normalizedBytes.append(contentsOf: header)
    var colorFrequencies = [UInt16: Int]()
    var unmaskedPixelCount = 0

    for y in 0..<image.height {
        for x in 0..<image.width {
            let offset = y * bytesPerRow + x * 4
            let isMarkerPixel =
                x >= resolvedNormalization.markerMaskMinX
                && x < resolvedNormalization.markerMaskMaxX
                && y >= resolvedNormalization.markerMaskMinY
                && y < resolvedNormalization.markerMaskMaxY
            if isMarkerPixel {
                normalizedBytes.append(contentsOf: [0, 0, 0, 0])
                continue
            }

            let red = pixels[offset]
            let green = pixels[offset + 1]
            let blue = pixels[offset + 2]
            let alpha = pixels[offset + 3]
            normalizedBytes.append(red)
            normalizedBytes.append(green)
            normalizedBytes.append(blue)
            normalizedBytes.append(alpha)
            unmaskedPixelCount += 1

            let quantized = UInt16(red >> 3) << 10
                | UInt16(green >> 3) << 5
                | UInt16(blue >> 3)
            colorFrequencies[quantized, default: 0] += 1
        }
    }

    let dominantPixelCount = colorFrequencies.values.max() ?? 0
    let nonDominantPixelCount = max(
        0,
        unmaskedPixelCount - dominantPixelCount
    )
    let minimumContentPixels = max(32, unmaskedPixelCount / 2_000)
    let contentEvidence = BenchmarkCompositedContentEvidence(
        normalizedFingerprintSHA256: digestHex(Data(normalizedBytes)),
        normalizedRGBA8Pixels:
            Array(normalizedBytes.dropFirst(header.count)),
        pixelWidth: image.width,
        pixelHeight: image.height,
        unmaskedPixelCount: unmaskedPixelCount,
        distinctQuantizedColorCount: colorFrequencies.count,
        nonDominantPixelCount: nonDominantPixelCount,
        hasNonblankContent: nonDominantPixelCount >= minimumContentPixels,
        hasNonuniformContent: colorFrequencies.count > 1
    )
    return BenchmarkCompositedCaptureEvidence(
        markerVisible: markerVisible,
        content: contentEvidence,
        normalization: resolvedNormalization,
        image: image
    )
}
@MainActor
func exactClientContentBounds(
    for window: NSWindow,
    on screen: NSScreen,
    evidence: BenchmarkWindowServerEvidence
) -> CGRect? {
    guard let screenNumber = screen.deviceDescription[
        NSDeviceDescriptionKey("NSScreenNumber")
    ] as? NSNumber,
          CGDirectDisplayID(screenNumber.uint32Value) == evidence.displayID else {
        return nil
    }
    let displayBounds = CGDisplayBounds(evidence.displayID)
    let content = window.contentRect(forFrameRect: window.frame)

    // AppKit screen coordinates are bottom-left based; CoreGraphics and
    // ScreenCaptureKit display coordinates are top-left based. Convert the
    // exact AppKit client rect instead of inferring insets from kCGWindowBounds,
    // whose server-side rectangle may include the window shadow.
    var bounds = CGRect(
        x: displayBounds.minX + content.minX - screen.frame.minX,
        y: displayBounds.minY + screen.frame.maxY - content.maxY,
        width: content.width,
        height: content.height
    )
    // Remove the client-area edge so rounded frame pixels cannot admit
    // titlebar, shadow, or wallpaper into the normalized content evidence.
    bounds = bounds.insetBy(dx: 1, dy: 1)
    guard bounds.width > 0,
          bounds.height > 0,
          evidence.bounds.insetBy(dx: -0.5, dy: -0.5).contains(bounds) else {
        return nil
    }
    return bounds
}

func captureCompositedClientContent(
    evidence: BenchmarkWindowServerEvidence,
    clientContentBounds: CGRect,
    markerColors: [BenchmarkVisibilityMarkerColor]? = nil,
    normalization: BenchmarkCompositedContentNormalization? = nil
) async throws -> BenchmarkCompositedCaptureEvidence {
    try await withBenchmarkDeadline(
        "ScreenCaptureKit composited client-content capture"
    ) {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false,
            onScreenWindowsOnly: true
        )
        guard let display = content.displays.first(where: {
            $0.displayID == evidence.displayID
        }) else {
            throw BenchmarkFailure.message(
                "ScreenCaptureKit could not resolve display \(evidence.displayID)"
            )
        }
        let sourceRect = clientContentBounds.offsetBy(
            dx: -display.frame.minX,
            dy: -display.frame.minY
        )
        guard sourceRect.minX >= 0,
              sourceRect.minY >= 0,
              sourceRect.maxX <= display.frame.width,
              sourceRect.maxY <= display.frame.height else {
            throw BenchmarkFailure.message(
                "exact client-content bounds escape the ScreenCaptureKit display"
            )
        }

        let filter = SCContentFilter(display: display, excludingWindows: [])
        let configuration = SCStreamConfiguration()
        configuration.sourceRect = sourceRect
        let horizontalScale = Double(display.width) / display.frame.width
        let verticalScale = Double(display.height) / display.frame.height
        configuration.width = max(
            1,
            Int(ceil(sourceRect.width * horizontalScale))
        )
        configuration.height = max(
            1,
            Int(ceil(sourceRect.height * verticalScale))
        )
        configuration.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter,
            configuration: configuration
        )
        guard let capture = normalizedCompositedContentEvidence(
            image,
            markerColors: markerColors,
            normalization: normalization
        ) else {
            throw BenchmarkFailure.message(
                "composited client-content capture could not produce exact "
                    + "normalized content evidence"
            )
        }
        return capture
    }
}
func captureAuthorizedWindowPixels(
    windowID: CGWindowID,
    pixelWidth: Int,
    pixelHeight: Int
) async throws -> Bool {
    try await withBenchmarkDeadline("ScreenCaptureKit authorized composited-window capture") {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false,
            onScreenWindowsOnly: true
        )
        guard let sharedWindow = content.windows.first(where: { $0.windowID == windowID }) else {
            throw BenchmarkFailure.message(
                "ScreenCaptureKit could not resolve benchmark window \(windowID)"
            )
        }
        let filter = SCContentFilter(desktopIndependentWindow: sharedWindow)
        let configuration = SCStreamConfiguration()
        configuration.width = pixelWidth
        configuration.height = pixelHeight
        configuration.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter,
            configuration: configuration
        )
        return image.width > 0 && image.height > 0
    }
}

@MainActor
func benchmarkVerifyAuthorizedWindowPixels(_ window: NSWindow) async throws -> Bool {
    guard CGPreflightScreenCaptureAccess() else { return false }
    let scale = window.screen?.backingScaleFactor ?? 1
    return try await captureAuthorizedWindowPixels(
        windowID: CGWindowID(window.windowNumber),
        pixelWidth: max(1, Int(window.frame.width * scale)),
        pixelHeight: max(1, Int(window.frame.height * scale))
    )
}

@MainActor
func benchmarkObserveOnScreenPaint(
    _ window: NSWindow,
    contentDrawProbe: BenchmarkDrawCompletionProbe? = nil,
    onPresented:
        (@MainActor (ContinuousClock.Instant, UInt64) -> Void)? = nil
) async throws -> OnScreenPaintObservation {
    guard let screen = window.screen ?? NSScreen.main,
          let contentView = window.contentView else {
        throw BenchmarkFailure.message(
            "benchmark window has no display or drawable client content"
        )
    }

    let ownedDrawProbe: BenchmarkDrawCompletionProbe?
    let drawProbe: BenchmarkDrawCompletionProbe
    if let contentDrawProbe {
        ownedDrawProbe = nil
        drawProbe = contentDrawProbe
    } else {
        let probe = BenchmarkDrawCompletionProbe(frame: contentView.bounds)
        probe.autoresizingMask = [.width, .height]
        contentView.addSubview(probe, positioned: .above, relativeTo: nil)
        ownedDrawProbe = probe
        drawProbe = probe
    }
    defer {
        ownedDrawProbe?.removeFromSuperview()
    }

    // This compatibility helper is an explicit hidden-backing-store -> visible
    // redraw proof. Measured §31.1 code uses the action-wrapping API directly,
    // so capture setup is outside its timing and a later marker redraw is never
    // mislabeled as the production DOM/SRUI presentation boundary.
    if window.isVisible {
        window.orderOut(nil)
        let hiddenDeadline = Date().addingTimeInterval(2)
        while exactWindowServerEvidence(for: window, on: screen) != nil,
              Date() < hiddenDeadline {
            try await Task.sleep(for: .milliseconds(1))
        }
        guard window.isVisible == false,
              exactWindowServerEvidence(for: window, on: screen) == nil else {
            throw BenchmarkFailure.message(
                "explicit redraw preparation could not hide its prior window"
            )
        }
    }

    let drawCountBeforeSubmission = drawProbe.drawCount
    let measurement = try await benchmarkMeasureExplicitCompositedPaint(
        on: screen,
        action: {
            drawProbe.prepareVisibilityMarker()
            contentView.layoutSubtreeIfNeeded()
            contentView.needsDisplay = true
            drawProbe.needsDisplay = true
            window.displayIfNeeded()
            CATransaction.flush()
            return BenchmarkExplicitPaintTarget(
                window: window,
                targetView: contentView
            )
        },
        onPresented: onPresented
    )
    guard drawProbe.drawCount > drawCountBeforeSubmission else {
        throw BenchmarkFailure.message(
            "explicit content draw marker did not complete before presentation"
        )
    }
    return measurement.observation
}
