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
func benchmarkTrace(_ message: String) {
    guard ProcessInfo.processInfo.environment["SRUI_BENCHMARK_TRACE"] == "1" else {
        return
    }
    FileHandle.standardError.write(Data("\(message)\n".utf8))
}

@MainActor
func benchmarkPhase(_ message: String) {
    guard ProcessInfo.processInfo.environment["SRUI_BENCHMARK_PHASES"] == "1" else {
        return
    }
    FileHandle.standardError.write(Data("\(message)\n".utf8))
}
@MainActor
final class BenchmarkApplicationDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        benchmarkTrace("benchmark application termination request suppressed")
        return .terminateCancel
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}
@main
struct BenchmarkDriver {
    @MainActor
    static func main() {
        let arguments: Arguments
        var parentWatchdog: BenchmarkParentLivenessWatchdog?
        var cleanupProbeDescendant: pid_t?

        func cleanupCandidateLifetime() {
            stopCandidateCleanupProbeDescendant(cleanupProbeDescendant)
            cleanupProbeDescendant = nil
            parentWatchdog?.cancel()
            parentWatchdog = nil
        }

        do {
            arguments = try Arguments()
            if arguments.candidate != nil {
                guard let parentBirth = arguments.driverBirthUnixNanoseconds else {
                    throw BenchmarkFailure.message(
                        "renderer candidate has no parent birth identity"
                    )
                }
                if arguments.supervisedParent {
                    guard getppid() == arguments.driverPID,
                          getpgrp() == arguments.driverPID,
                          benchmarkProcessMatchesIdentity(
                              pid: arguments.driverPID,
                              birthUnixNanoseconds: parentBirth
                          ) else {
                        throw BenchmarkFailure.message(
                            "renderer candidate is not inside its exact supervisor group"
                        )
                    }
                } else {
                    guard setpgid(0, 0) == 0, getpgrp() == getpid() else {
                        throw BenchmarkFailure.message(
                            "renderer candidate could not establish process-group ownership: errno \(errno)"
                        )
                    }
                    let watchdog = try BenchmarkParentLivenessWatchdog(
                        parentPID: arguments.driverPID,
                        parentBirthUnixNanoseconds: parentBirth
                    )
                    try watchdog.start()
                    parentWatchdog = watchdog
                }
                cleanupProbeDescendant =
                    try startCandidateCleanupProbeDescendantIfRequested()
            }
        } catch {
            cleanupCandidateLifetime()
            FileHandle.standardError.write(
                Data("BenchmarkDriver failed: \(error)\n".utf8)
            )
            Darwin.exit(EXIT_FAILURE)
        }
        defer {
            cleanupCandidateLifetime()
        }
        // Candidate process-group ownership and parent-birth supervision are established above,
        // synchronously, before this first AppKit access. Full measurements must be eligible
        // to become the active foreground application so AppKit and WindowServer can truthfully
        // report visible, non-occluded presentation. Smoke keeps the unobtrusive accessory policy.
        let application = NSApplication.shared
        let applicationDelegate = BenchmarkApplicationDelegate()
        application.delegate = applicationDelegate
        let activationPolicy: NSApplication.ActivationPolicy =
            arguments.profile == "full" ? .regular : .accessory
        _ = application.setActivationPolicy(activationPolicy)
        guard application.activationPolicy() == activationPolicy else {
            cleanupCandidateLifetime()
            FileHandle.standardError.write(
                Data(
                    "BenchmarkDriver failed: could not establish \(activationPolicy) activation policy\n".utf8
                )
            )
            Darwin.exit(EXIT_FAILURE)
        }
        application.finishLaunching()
        if arguments.profile == "full" {
            application.activate()
        }

        var failure: (any Error)?
        Task { @MainActor in
            do {
                try await run(arguments: arguments)
            } catch {
                failure = error
            }
            application.stop(nil)
            if let wakeEvent = NSEvent.otherEvent(
                with: .applicationDefined,
                location: .zero,
                modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: 0,
                context: nil,
                subtype: 0,
                data1: 0,
                data2: 0
            ) {
                application.postEvent(wakeEvent, atStart: false)
            }
        }
        application.run()
        application.delegate = nil
        withExtendedLifetime(applicationDelegate) {}
        if let failure {
            cleanupCandidateLifetime()
            FileHandle.standardError.write(
                Data("BenchmarkDriver failed: \(failure)\n".utf8)
            )
            Darwin.exit(EXIT_FAILURE)
        }
    }

    @MainActor
    private static func run(arguments: Arguments) async throws {
        let fixture = try JSONDecoder().decode(
            Fixture.self,
            from: Data(contentsOf: arguments.fixture)
        )
        let fixtureOperations = try operations(for: fixture)
        let iterations = arguments.profile == "full" ? 20 : 3
        let fullPaint = arguments.profile == "full"

        if ProcessInfo.processInfo.environment[
            "SRUI_BENCHMARK_WINDOW_ISOLATION_SELF_TEST"
        ] == "1" {
            guard fullPaint else {
                throw BenchmarkFailure.message(
                    "window-isolation self-test requires --profile full"
                )
            }
            try await benchmarkVerifyWindowServerIsolation()
            // This diagnostic intentionally exits before any benchmark
            // measurement so its temporary windows and activation state cannot
            // contaminate renderer timing or pixels.
            return
        }

        if let candidate = arguments.candidate {
            if ProcessInfo.processInfo.environment[
                "SRUI_BENCHMARK_FORCE_CANDIDATE_INTERNAL_FAILURE"
            ] == "1" {
                throw BenchmarkFailure.message(
                    "forced renderer candidate internal failure after descendant setup"
                )
            }
            let allocationControl: AllocationCaptureControl?
            if let directory = arguments.allocationControlDirectory,
               let targetRole = arguments.allocationTargetRole {
                guard candidate == "webkit" || targetRole == "host" else {
                    throw BenchmarkFailure.message(
                        "native allocation capture supports only the host role"
                    )
                }
                allocationControl = try AllocationCaptureControl(
                    directory: directory,
                    targetRole: targetRole
                )
            } else {
                allocationControl = nil
            }
            let result: RendererCandidateResult
            switch candidate {
            case "srui":
                result = try await runSRUICandidate(
                    fixture: fixture,
                    operations: fixtureOperations,
                    iterations: iterations,
                    fullPaint: fullPaint,
                    driverPID: arguments.driverPID,
                    allocationControl: allocationControl
                )
            case "webkit":
                result = try await runWebCandidate(
                    fixture: fixture,
                    iterations: iterations,
                    fullPaint: fullPaint,
                    driverPID: arguments.driverPID,
                    allocationControl: allocationControl
                )
            default:
                throw BenchmarkFailure.message("unknown renderer candidate \(candidate)")
            }
            guard let candidateIdentity = benchmarkProcessIdentity(pid: getpid()) else {
                throw BenchmarkFailure.message(
                    "renderer candidate validation identity was unavailable"
                )
            }
            try validateRendererCandidateAttribution(
                result,
                expectedCandidate: candidate,
                expectedDriverPID: arguments.driverPID,
                expectedHostPID: getpid(),
                expectedHostBirthUnixNanoseconds:
                    candidateIdentity.birthUnixNanoseconds,
                allocationTargetRole: arguments.allocationTargetRole
            )
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            try encoder.encode(result).write(to: arguments.output)
            return
        }

        let canonicalTransaction = try progressiveTransactionPlan(
            fixture: fixture,
            operations: fixtureOperations
        ).canonicalBytes

        var sections = [Section]()
        var rendererProcessAttribution = [RendererProcessAttribution]()
        if arguments.onlySection == nil || arguments.onlySection == "31.1" {
            let result = try localRenderer(
                fixture: fixture,
                fixtureURL: arguments.fixture,
                profile: arguments.profile
            )
            sections.append(result.section)
            rendererProcessAttribution = result.attributions
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.3" {
            sections.append(
                try await mutationAndCadence(
                    fixtureOperations: fixtureOperations,
                    iterations: iterations,
                    fullPaint: fullPaint
                )
            )
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.4" {
            sections.append(
                try await networkAndLocalInteraction(
                    fixtureOperations: fixtureOperations,
                    iterations: max(5, iterations),
                    fullPaint: fullPaint
                )
            )
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.5" {
            sections.append(
                try await reconnect(
                    iterations: iterations,
                    fixtureOperations: fixtureOperations
                )
            )
        }
        if arguments.onlySection == nil || arguments.onlySection == "31.6" {
            sections.append(
                try await terminal(
                    iterations: max(10, iterations),
                    fullPaint: fullPaint
                )
            )
        }

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(
            Output(
                artifacts: Artifacts(
                    canonicalTransactionSHA256: digestHex(canonicalTransaction),
                    canonicalTransactionBytes: canonicalTransaction.count,
                    rendererProcessAttribution: rendererProcessAttribution
                ),
                sections: sections
            )
        ).write(to: arguments.output)
    }
}
