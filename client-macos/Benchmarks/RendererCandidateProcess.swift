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
func startCandidateCleanupProbeDescendantIfRequested() throws -> pid_t? {
    guard let identityPath = ProcessInfo.processInfo.environment[
        "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH"
    ] else {
        return nil
    }

    var processID: pid_t = 0
    var arguments: [UnsafeMutablePointer<CChar>?] = [
        strdup("sleep"),
        strdup("300"),
        nil,
    ]
    defer {
        for argument in arguments where argument != nil {
            free(argument)
        }
    }

    var fileActions: posix_spawn_file_actions_t?
    let fileActionsStatus = posix_spawn_file_actions_init(&fileActions)
    guard fileActionsStatus == 0 else {
        throw BenchmarkFailure.message(
            "cleanup probe file-actions init failed: errno "
                + "\(fileActionsStatus)"
        )
    }
    defer {
        _ = posix_spawn_file_actions_destroy(&fileActions)
    }
    let redirects: [(descriptor: Int32, flags: Int32)] = [
        (STDIN_FILENO, O_RDONLY),
        (STDOUT_FILENO, O_WRONLY),
        (STDERR_FILENO, O_WRONLY),
    ]
    for redirect in redirects {
        let status = posix_spawn_file_actions_addopen(
            &fileActions,
            redirect.descriptor,
            "/dev/null",
            redirect.flags,
            mode_t(0)
        )
        guard status == 0 else {
            throw BenchmarkFailure.message(
                "cleanup probe /dev/null redirect failed for fd "
                    + "\(redirect.descriptor): errno \(status)"
            )
        }
    }

    let spawnStatus = arguments.withUnsafeMutableBufferPointer { buffer in
        posix_spawn(
            &processID,
            "/bin/sleep",
            &fileActions,
            nil,
            buffer.baseAddress,
            environ
        )
    }
    guard spawnStatus == 0, processID > 0 else {
        throw BenchmarkFailure.message(
            "cleanup probe descendant spawn failed: errno \(spawnStatus)"
        )
    }

    var descendantHandedOff = false
    defer {
        if descendantHandedOff == false {
            stopCandidateCleanupProbeDescendant(processID)
        }
    }
    let deadline = Date().addingTimeInterval(2)
    var identity = benchmarkProcessIdentity(pid: processID)
    while identity == nil,
          Darwin.kill(processID, 0) == 0,
          Date() < deadline {
        _ = RunLoop.current.run(
            mode: .default,
            before: min(deadline, Date().addingTimeInterval(0.005))
        )
        identity = benchmarkProcessIdentity(pid: processID)
    }
    guard let identity,
          getpgid(processID) == getpid(),
          benchmarkProcessMatchesIdentity(
              pid: processID,
              birthUnixNanoseconds: identity.birthUnixNanoseconds
          ) else {
        throw BenchmarkFailure.message(
            "cleanup probe descendant did not join the candidate process group"
        )
    }

    let encodedIdentity = try JSONEncoder().encode(identity)
    if let observerPath = ProcessInfo.processInfo.environment[
        "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_OBSERVER_PATH"
    ] {
        try encodedIdentity.write(
            to: URL(fileURLWithPath: observerPath),
            options: .atomic
        )
    }
    try encodedIdentity.write(
        to: URL(fileURLWithPath: identityPath),
        options: .atomic
    )
    descendantHandedOff = true
    return processID
}
@MainActor
func stopCandidateCleanupProbeDescendant(_ processID: pid_t?) {
    guard let processID else { return }
    if Darwin.kill(processID, 0) == 0 {
        _ = Darwin.kill(processID, SIGKILL)
    }
    var status: Int32 = 0
    while waitpid(processID, &status, 0) == -1, errno == EINTR {}
}

func validateRendererCandidateAttribution(
    _ result: RendererCandidateResult,
    expectedCandidate: String,
    expectedDriverPID: Int32,
    expectedHostPID: Int32,
    expectedHostBirthUnixNanoseconds: UInt64
) throws {
    let attribution = result.attribution
    let attributedPIDs = Set([attribution.hostPID] + attribution.helperPIDs)
    let helperPIDs = Set(attribution.helperPIDs)
    let identityPIDs = Set(attribution.processIdentities.map(\.pid))
    let attributedHost = attribution.processIdentities.first {
        $0.pid == expectedHostPID
    }

    guard result.candidate == expectedCandidate,
          attribution.candidate == expectedCandidate,
          attribution.hostPID == expectedHostPID,
          attribution.driverPID == expectedDriverPID,
          attributedPIDs == identityPIDs,
          helperPIDs.contains(expectedHostPID) == false,
          Set(attribution.helperPIDs).count == attribution.helperPIDs.count,
          result.resourceAttributionComplete,
          attribution.processIdentities.allSatisfy({
              $0.birthUnixNanoseconds > 0
                  && $0.observedAliveThroughUnixNanoseconds
                      >= $0.birthUnixNanoseconds
          }),
          attribution.measurementIntervals.isEmpty == false,
          attribution.measurementIntervals.allSatisfy({
              $0.startedUnixNanoseconds >= attribution.startedUnixNanoseconds
                  && $0.startedUnixNanoseconds < $0.endedUnixNanoseconds
                  && $0.endedUnixNanoseconds <= attribution.endedUnixNanoseconds
          }),
          attributedHost?.birthUnixNanoseconds
            == expectedHostBirthUnixNanoseconds,
          attributedHost?.observedAliveThroughUnixNanoseconds
            ?? 0 >= (attribution.measurementIntervals.last?
                .endedUnixNanoseconds ?? UInt64.max) else {
        throw BenchmarkFailure.message(
            "renderer candidate process attribution mismatch"
        )
    }
}

@MainActor
func runCandidateSubprocess(
    name: String,
    fixture: URL,
    profile: String
) throws -> RendererCandidateResult {
    let fileManager = FileManager.default
    let temporary = fileManager.temporaryDirectory
        .appendingPathComponent("srui-benchmark-\(name)-\(UUID().uuidString).json")
    let standardOutputURL = temporary.appendingPathExtension("stdout")
    let standardErrorURL = temporary.appendingPathExtension("stderr")
    guard fileManager.createFile(atPath: standardOutputURL.path, contents: nil),
          fileManager.createFile(atPath: standardErrorURL.path, contents: nil) else {
        throw BenchmarkFailure.message("renderer candidate log files could not be created")
    }
    let standardOutput = try FileHandle(forWritingTo: standardOutputURL)
    let standardError = try FileHandle(forWritingTo: standardErrorURL)
    defer {
        try? standardOutput.close()
        try? standardError.close()
        try? fileManager.removeItem(at: temporary)
        try? fileManager.removeItem(at: standardOutputURL)
        try? fileManager.removeItem(at: standardErrorURL)
    }

    let executablePath = CommandLine.arguments[0]
    let executable = URL(
        fileURLWithPath: executablePath,
        relativeTo: URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    ).standardizedFileURL
    guard let driverIdentity = benchmarkProcessIdentity(pid: getpid()) else {
        throw BenchmarkFailure.message("benchmark driver birth identity was unavailable")
    }
    let process = Process()
    process.executableURL = executable
    process.arguments = [
        "--fixture", fixture.path,
        "--output", temporary.path,
        "--profile", profile,
        "--candidate", name,
        "--driver-pid", String(getpid()),
        "--driver-birth-unix-ns", String(driverIdentity.birthUnixNanoseconds),
    ]
    process.standardOutput = standardOutput
    process.standardError = standardError
    try process.run()
    let processID = process.processIdentifier

    func waitForCandidateExit(until deadline: Date) -> Bool {
        while process.isRunning && Date() < deadline {
            _ = RunLoop.current.run(
                mode: .default,
                before: min(deadline, Date().addingTimeInterval(0.01))
            )
        }
        return process.isRunning == false
    }
    func forceStopAndReap(
        expectedIdentity: RendererProcessIdentity?
    ) -> String? {
        guard process.isRunning else { return nil }
        var signalled = false
        if let expectedIdentity {
            let signalDeadline = Date().addingTimeInterval(0.25)
            while process.isRunning, signalled == false, Date() < signalDeadline {
                signalled = benchmarkSignalLiveProcessGroup(
                    processID,
                    expectedBirthUnixNanoseconds:
                        expectedIdentity.birthUnixNanoseconds,
                    signal: SIGKILL
                )
                if signalled == false {
                    _ = RunLoop.current.run(
                        mode: .default,
                        before: min(
                            signalDeadline,
                            Date().addingTimeInterval(0.005)
                        )
                    )
                }
            }
        }
        // Never fall back to signaling the numeric PID: failed birth/group
        // validation may mean that PID now belongs to an unrelated process.
        let reaped = waitForCandidateExit(until: Date().addingTimeInterval(5))
        var failures = [String]()
        if reaped == false {
            failures.append("candidate was not reaped")
            if signalled == false {
                failures.append(
                    "candidate process-group ownership could not be validated"
                )
            }
        }
        return failures.isEmpty ? nil : failures.joined(separator: "; ")
    }

    var cleanupIdentity: RendererProcessIdentity?
    var launchedIdentity: RendererProcessIdentity?
    do {
        let identityDeadline = Date().addingTimeInterval(1)
        cleanupIdentity = benchmarkProcessIdentity(pid: processID)
        while cleanupIdentity == nil,
              process.isRunning,
              Date() < identityDeadline {
            _ = RunLoop.current.run(
                mode: .default,
                before: min(identityDeadline, Date().addingTimeInterval(0.005))
            )
            cleanupIdentity = benchmarkProcessIdentity(pid: processID)
        }
        guard let cleanupIdentity else {
            throw BenchmarkFailure.message(
                "renderer candidate \(name) cleanup identity was unavailable"
            )
        }
        if let identityPath = ProcessInfo.processInfo.environment[
            "SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH"
        ] {
            try JSONEncoder().encode(cleanupIdentity).write(
                to: URL(fileURLWithPath: identityPath),
                options: .atomic
            )
        }
        let forceIdentityFailure =
            ProcessInfo.processInfo.environment[
                "SRUI_BENCHMARK_FORCE_CANDIDATE_IDENTITY_FAILURE"
            ] == "1"
        if forceIdentityFailure,
           let descendantIdentityPath = ProcessInfo.processInfo.environment[
               "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH"
           ] {
            let descendantURL = URL(fileURLWithPath: descendantIdentityPath)
            let descendantDeadline = Date().addingTimeInterval(2)
            var descendantIdentity: RendererProcessIdentity?
            while descendantIdentity == nil,
                  process.isRunning,
                  Date() < descendantDeadline {
                descendantIdentity = try? JSONDecoder().decode(
                    RendererProcessIdentity.self,
                    from: Data(contentsOf: descendantURL)
                )
                if descendantIdentity == nil {
                    _ = RunLoop.current.run(
                        mode: .default,
                        before: min(
                            descendantDeadline,
                            Date().addingTimeInterval(0.005)
                        )
                    )
                }
            }
            guard let descendantIdentity,
                  getpgid(descendantIdentity.pid) == processID,
                  benchmarkProcessMatchesIdentity(
                      pid: descendantIdentity.pid,
                      birthUnixNanoseconds: descendantIdentity.birthUnixNanoseconds
                  ) else {
                throw BenchmarkFailure.message(
                    "renderer candidate \(name) cleanup probe descendant was unavailable"
                )
            }
        }
        launchedIdentity = forceIdentityFailure ? nil : cleanupIdentity
        guard let launchedIdentity else {
            throw BenchmarkFailure.message(
                "renderer candidate \(name) birth identity was unavailable"
            )
        }

        let deadline = Date().addingTimeInterval(profile == "full" ? 240 : 60)
        guard waitForCandidateExit(until: deadline) else {
            throw BenchmarkFailure.message("renderer candidate \(name) timed out")
        }

        try? standardOutput.synchronize()
        try? standardError.synchronize()
        let candidateStandardError = (try? String(
            contentsOf: standardErrorURL,
            encoding: .utf8
        )) ?? ""
        let candidateStandardOutput = (try? String(
            contentsOf: standardOutputURL,
            encoding: .utf8
        )) ?? ""
        let candidateDiagnostics = candidateStandardError.isEmpty
            ? candidateStandardOutput
            : candidateStandardError
        guard process.terminationReason == .exit, process.terminationStatus == 0 else {
            let diagnosticSuffix = candidateDiagnostics.isEmpty
                ? ""
                : ": \(candidateDiagnostics.suffix(4_000))"
            throw BenchmarkFailure.message(
                "renderer candidate \(name) exited with status \(process.terminationStatus)"
                    + diagnosticSuffix
            )
        }
        let result = try JSONDecoder().decode(
            RendererCandidateResult.self,
            from: Data(contentsOf: temporary)
        )
        try validateRendererCandidateAttribution(
            result,
            expectedCandidate: name,
            expectedDriverPID: getpid(),
            expectedHostPID: processID,
            expectedHostBirthUnixNanoseconds:
                launchedIdentity.birthUnixNanoseconds
        )
        return result
    } catch {
        if let cleanupFailure = forceStopAndReap(
            expectedIdentity: cleanupIdentity ?? launchedIdentity
        ) {
            throw BenchmarkFailure.message(
                "\(error); candidate cleanup failed: \(cleanupFailure)"
            )
        }
        throw error
    }
}

struct LocalRendererResult {
    let section: Section
    let attributions: [RendererProcessAttribution]
}