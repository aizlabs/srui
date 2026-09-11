import AppKit
import CryptoKit
import Darwin
import Foundation
import Protocol
import RendererAppKit
import Resources
import SemanticModel
import Session
import SwiftProtobuf
import Terminal
import TransportSSH
import WebKit

@MainActor
func localRenderer(
    fixture: Fixture,
    fixtureURL: URL,
    profile: String
) throws -> LocalRendererResult {
    let fullPaint = profile == "full"
    let originalPointerLocation = fullPaint
        ? try benchmarkParkPointerOutsideMeasurementROI(
            on: try benchmarkMainScreen(),
            side: .left
        ) : nil
    defer {
        if let originalPointerLocation {
            benchmarkRestorePointer(originalPointerLocation)
        }
    }

    let srui = try runCandidateSubprocess(
        name: "srui",
        fixture: fixtureURL,
        profile: profile
    )
    let web = try runCandidateSubprocess(
        name: "webkit",
        fixture: fixtureURL,
        profile: profile
    )
    let nativeFirstName = fullPaint
        ? "SRUI first on-screen paint crossing display refresh"
        : "SRUI first offscreen raster fallback"
    let nativeCompleteName = fullPaint
        ? "SRUI complete on-screen paint crossing display refresh"
        : "SRUI complete offscreen raster fallback"
    let webFirstName = fullPaint
        ? "WKWebView first on-screen paint crossing display refresh"
        : "WKWebView first offscreen snapshot fallback"
    let webCompleteName = fullPaint
        ? "WKWebView complete on-screen paint crossing display refresh"
        : "WKWebView complete offscreen snapshot fallback"
    let captureAuthorization = srui.captureAuthorization && web.captureAuthorization

    func paintEvidencePassed(_ result: RendererCandidateResult) -> Bool {
        let expectedPaintCompletions = result.expectedSampleCount * 2
        return result.expectedSampleCount > 0
            && result.firstPaint.count == result.expectedSampleCount
            && result.completePaint.count == result.expectedSampleCount
            && result.presentationCompletions == expectedPaintCompletions
            && result.contentPresentationPassed
            && (!fullPaint || (
                result.captureAuthorization
                    && result.pixelCaptureCompletions == expectedPaintCompletions
            ))
    }

    let section = Section(
        id: "31.1",
        name: "Local renderer",
        sampleCounts: [
            "macos.srui.render": srui.firstPaint.count,
            "macos.webkit.render": web.firstPaint.count,
        ],
        metrics: [
            metric(
                nativeFirstName,
                p50(srui.firstPaint),
                id: "srui.first_paint"
            ),
            metric(
                nativeFirstName,
                percentile(srui.firstPaint, 0.95),
                "ms",
                "p95",
                id: "srui.first_paint"
            ),
            metric(
                nativeCompleteName,
                p50(srui.completePaint),
                id: "srui.complete_paint"
            ),
            metric(
                nativeCompleteName,
                percentile(srui.completePaint, 0.95),
                "ms",
                "p95",
                id: "srui.complete_paint"
            ),
            metric(
                nativeCompleteName,
                percentile(srui.completePaint, 0.99),
                "ms",
                "p99",
                id: "srui.complete_paint"
            ),
            metric(
                "SRUI candidate process CPU time",
                p50(srui.cpuTime),
                id: "srui.cpu"
            ),
            metric(
                "SRUI candidate process CPU time",
                percentile(srui.cpuTime, 0.95),
                "ms",
                "p95",
                id: "srui.cpu"
            ),
            metric(
                "SRUI host-process net live allocation block delta",
                p50(srui.hostNetLiveAllocationBlockDelta),
                "blocks",
                "p50",
                id: "srui.host_net_live_allocation_blocks"
            ),
            metric(
                "SRUI host-process net live allocation block delta",
                percentile(srui.hostNetLiveAllocationBlockDelta, 0.95),
                "blocks",
                "p95",
                id: "srui.host_net_live_allocation_blocks"
            ),
            metric(
                "SRUI host-process net live allocation block delta",
                percentile(srui.hostNetLiveAllocationBlockDelta, 0.99),
                "blocks",
                "p99",
                id: "srui.host_net_live_allocation_blocks"
            ),
            metric(
                "SRUI host-process net live allocation byte delta",
                p50(srui.hostNetLiveAllocationByteDelta),
                "bytes",
                "p50",
                id: "srui.host_net_live_allocation_bytes"
            ),
            metric(
                "SRUI host-process net live allocation byte delta",
                percentile(srui.hostNetLiveAllocationByteDelta, 0.95),
                "bytes",
                "p95",
                id: "srui.host_net_live_allocation_bytes"
            ),
            metric(
                "SRUI host-process net live allocation byte delta",
                percentile(srui.hostNetLiveAllocationByteDelta, 0.99),
                "bytes",
                "p99",
                id: "srui.host_net_live_allocation_bytes"
            ),
            metric(
                "SRUI host allocated footprint growth",
                srui.allocatedFootprintGrowthMiB,
                "MiB",
                "p50",
                id: "srui.process_footprint_growth"
            ),
            metric(
                "SRUI maximum concurrently sampled process footprint",
                srui.processFootprintPeak.max() ?? -1,
                "MiB",
                "max",
                id: "srui.process_footprint_peak"
            ),
            metric(
                webFirstName,
                p50(web.firstPaint),
                id: "webkit.first_paint"
            ),
            metric(
                webFirstName,
                percentile(web.firstPaint, 0.95),
                "ms",
                "p95",
                id: "webkit.first_paint"
            ),
            metric(
                webCompleteName,
                p50(web.completePaint),
                id: "webkit.complete_paint"
            ),
            metric(
                webCompleteName,
                percentile(web.completePaint, 0.95),
                "ms",
                "p95",
                id: "webkit.complete_paint"
            ),
            metric(
                "WKWebView host plus attributed helper CPU time",
                p50(web.cpuTime),
                id: "webkit.cpu"
            ),
            metric(
                "WKWebView comparison host-process net live allocation block delta",
                p50(web.hostNetLiveAllocationBlockDelta),
                "blocks",
                "p50",
                id: "webkit.host_net_live_allocation_blocks"
            ),
            metric(
                "WKWebView comparison host-process net live allocation block delta",
                percentile(web.hostNetLiveAllocationBlockDelta, 0.95),
                "blocks",
                "p95",
                id: "webkit.host_net_live_allocation_blocks"
            ),
            metric(
                "WKWebView comparison host-process net live allocation block delta",
                percentile(web.hostNetLiveAllocationBlockDelta, 0.99),
                "blocks",
                "p99",
                id: "webkit.host_net_live_allocation_blocks"
            ),
            metric(
                "WKWebView comparison host-process net live allocation byte delta",
                p50(web.hostNetLiveAllocationByteDelta),
                "bytes",
                "p50",
                id: "webkit.host_net_live_allocation_bytes"
            ),
            metric(
                "WKWebView comparison host-process net live allocation byte delta",
                percentile(web.hostNetLiveAllocationByteDelta, 0.95),
                "bytes",
                "p95",
                id: "webkit.host_net_live_allocation_bytes"
            ),
            metric(
                "WKWebView comparison host-process net live allocation byte delta",
                percentile(web.hostNetLiveAllocationByteDelta, 0.99),
                "bytes",
                "p99",
                id: "webkit.host_net_live_allocation_bytes"
            ),
            metric(
                "WKWebView host plus helpers allocated footprint growth",
                web.allocatedFootprintGrowthMiB,
                "MiB",
                "p50",
                id: "webkit.process_footprint_growth"
            ),
            metric(
                "WKWebView maximum concurrently sampled host-plus-helper footprint",
                web.processFootprintPeak.max() ?? -1,
                "MiB",
                "max",
                id: "webkit.process_footprint_peak"
            ),
            metric(
                "SRUI representation",
                Double(srui.representationBytes),
                "bytes",
                "exact",
                id: "representation.srui_bytes"
            ),
            metric(
                "HTML representation",
                Double(web.representationBytes),
                "bytes",
                "exact",
                id: "representation.html_bytes"
            ),
            metric(
                "screen capture authorization",
                captureAuthorization ? 1 : 0,
                "boolean",
                "exact",
                id: "paint.capture_authorization"
            ),
        ],
        assertions: [
            Assertion(
                id: "semantic_representation_parity",
                name: "representative fixture preserves exact parent, type, and property semantics",
                passed: srui.renderedNodeCount == fixture.nodes.count
                    && web.renderedNodeCount == fixture.nodes.count
                    && srui.semanticParityPassed
                    && srui.elementKindsPassed
                    && web.semanticParityPassed
                    && web.elementKindsPassed,
                detail: "expected=\(fixture.nodes.count), native=\(srui.renderedNodeCount) semantic=\(srui.semanticParityPassed) controls=\(srui.elementKindsPassed), WebKit=\(web.renderedNodeCount) semantic=\(web.semanticParityPassed) elements-and-properties=\(web.elementKindsPassed)"
            ),
            Assertion(
                id: "paint_completion_observed",
                name: fullPaint
                    ? "candidate production state reaches a verified composited target-pixel frame"
                    : "candidate content draw completes in the offscreen raster fallback",
                passed: paintEvidencePassed(srui) && paintEvidencePassed(web),
                detail: "native presentations=\(srui.presentationCompletions)/\(srui.expectedSampleCount * 2), pixels=\(srui.pixelCaptureCompletions), authorized=\(srui.captureAuthorization), content=\(srui.contentPresentationDetail); WebKit presentations=\(web.presentationCompletions)/\(web.expectedSampleCount * 2), pixels=\(web.pixelCaptureCompletions), authorized=\(web.captureAuthorization), content=\(web.contentPresentationDetail); \(srui.paintCompletionMode); \(web.paintCompletionMode)"
            ),
            Assertion(
                id: "webkit_helpers_attributed",
                name: "WebKit helper resources use exact measured process attribution",
                passed: web.attribution.helperPIDs.isEmpty == false
                    && web.resourceAttributionComplete,
                detail: "host \(web.attribution.hostPID), helpers \(web.attribution.helperPIDs); no process-name matching"
            ),
            Assertion(
                id: "host_net_live_allocation_scope",
                name: "signed net live allocation samples have exact host-process scope",
                passed: srui.hostAllocationMeasurementScope
                        == sruiHostAllocationMeasurementScope
                    && web.hostAllocationMeasurementScope
                        == webKitHostAllocationMeasurementScope
                    && srui.hostNetLiveAllocationBlockDelta.count
                        == srui.cpuTime.count
                    && srui.hostNetLiveAllocationByteDelta.count
                        == srui.cpuTime.count
                    && web.hostNetLiveAllocationBlockDelta.count
                        == web.cpuTime.count
                    && web.hostNetLiveAllocationByteDelta.count
                        == web.cpuTime.count
                    && srui.cpuTime.isEmpty == false
                    && web.cpuTime.isEmpty == false,
                detail: "SRUI blocks=\(srui.hostNetLiveAllocationBlockDelta.count), bytes=\(srui.hostNetLiveAllocationByteDelta.count), scope=\(srui.hostAllocationMeasurementScope); WebKit control blocks=\(web.hostNetLiveAllocationBlockDelta.count), bytes=\(web.hostNetLiveAllocationByteDelta.count), scope=\(web.hostAllocationMeasurementScope)"
            ),
        ],
        notes: [
            fullPaint
                ? "Every accepted full-paint state requires an exact visible CGWindow, exact client/target geometry, unobscured z-order, and authorized nonblank/nonuniform ScreenCaptureKit pixels from one complete frame. Latency is action-start Mach time through that accepted frame's SCStream displayTime; callback receipt and pixel hashing are verifier metadata, not the presentation timestamp. The two reported timed states per logical sample produced native=\(srui.pixelCaptureCompletions) and WebKit=\(web.pixelCaptureCompletions) verified captures; capture_authorization=\(captureAuthorization). Warm/resource/peak passes are not counted in that metric. No permission request is issued."
                : "Smoke deliberately uses named offscreen AppKit bitmap and WKSnapshot fallbacks; it validates state and raster completion but makes no WindowServer, visibility, or composited-pixel claim.",
            "Each native logical sample uses four separately warmed/reset renderer instances: first-state visual latency, complete two-transaction visual latency, CPU/host-net-live-allocation/footprint-growth through production display submission, and sampler-only peak footprint. Each WebKit sample resets one warmed view between the same four disjoint workloads while preserving exact helper PID identities. Neither resource pass creates ScreenCaptureKit buffers, and the footprint sampler never runs in the CPU/allocation pass.",
            "Native first timing decodes and applies revision 0→1 through production initial attach. Native complete timing decodes/applies 0→1 and then applies 1→2 through production incremental apply. WebKit uses structurally equivalent first and complete DOM states. The measured instance is checked immediately after each accepted presentation for exact semantic/control or DOM state. Full mode additionally requires nonblank, nonuniform, equal-geometry client-content fingerprints that differ between first and complete states.",
            "Each candidate process group is birth-identity verified. WebKit CPU, footprint growth, and peak footprint aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs. Allocation samples are signed default-zone malloc_zone_statistics after-minus-before deltas: blocks_in_use and size_in_use describe net live state, not cumulative allocation traffic. Allocation scope: \(srui.hostAllocationMeasurementScope). Control scope: \(web.hostAllocationMeasurementScope). Peak footprint is the maximum simultaneous current-footprint sample at 1 ms cadence strictly inside its separate representative pass, not a sum of per-process lifetime maxima.",
            "Task 34 deliberately defers cumulative allocation-call and requested-byte counts. The supported malloc_history all-events export was rejected after the first real pre-workload SRUI snapshot expanded to 1,902,439,272 bytes; no value from that attempt entered this report. GitHub issue aizlabs/srui#48 tracks a benchmark-only Darwin allocator-interposition counter.",
            "The warmed WKWebView candidate and its host-only allocator deltas are comparison controls only; they are not the production SRUI renderer, do not cover WebKit helper-process allocations, and do not describe SRUI's native AppKit rendering path.",
        ]
    )
    return LocalRendererResult(
        section: section,
        attributions: [srui.attribution, web.attribution]
    )
}