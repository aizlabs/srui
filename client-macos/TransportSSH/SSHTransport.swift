//
// SSHTransport.swift
// TransportSSH
//
// Secure Shell (SSH) transport binding implementing §19 and §19.1 posture rules.
//

import Foundation
#if canImport(Darwin)
import Darwin
#endif

/// Thread-safe accumulator for stderr diagnostics and process lifecycle messages.
private final class StderrAccumulator: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String] = []

    func append(_ text: String) {
        lock.lock()
        defer { lock.unlock() }
        let splitLines = text.split(whereSeparator: \.isNewline).map(String.init)
        lines.append(contentsOf: splitLines)
        if lines.count > 100 {
            lines.removeFirst(lines.count - 100)
        }
    }

    func summary() -> String {
        lock.lock()
        defer { lock.unlock() }
        return lines.joined(separator: "\n")
    }
}

/// Actor implementing the SRUI secure SSH transport binding over an OpenSSH subsystem channel (§19, §19.1, §25).
public actor SSHTransport: Transport {
    public let configuration: SSHConfiguration

    private var process: Process?
    private var stdinPipe: Pipe?
    private var stdoutPipe: Pipe?
    private var stderrPipe: Pipe?

    private var isClosed = false
    private var isConnected = false
    private var readTask: Task<Void, Never>?
    private var stderrTask: Task<Void, Never>?
    private let stderrAccumulator = StderrAccumulator()

    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    public init(configuration: SSHConfiguration) {
        self.configuration = configuration
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation

        continuation.onTermination = { [weak self] _ in
            Task {
                await self?.close()
            }
        }
    }

    /// Convenience initializer with host and optional parameters.
    public init(
        host: String,
        port: UInt16? = nil,
        user: String? = nil,
        subsystem: String = "srui",
        identityFile: String? = nil,
        knownHostsFile: String? = nil,
        strictHostKeyChecking: StrictHostKeyCheckingMode = .yes,
        batchMode: Bool = false
    ) {
        self.init(
            configuration: SSHConfiguration(
                host: host,
                port: port,
                user: user,
                subsystem: subsystem,
                identityFile: identityFile,
                knownHostsFile: knownHostsFile,
                strictHostKeyChecking: strictHostKeyChecking,
                batchMode: batchMode
            )
        )
    }

    deinit {
        let proc = process
        if let proc, proc.isRunning {
            proc.terminate()
        }
    }

    /// Connects and launches the SSH subsystem process.
    public func connect() throws {
        guard !isConnected else { return }
        guard !isClosed else {
            throw TransportError.closed
        }

        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: configuration.sshBinaryPath)
        proc.arguments = configuration.buildArguments()

        let inPipe = Pipe()
        let outPipe = Pipe()
        let errPipe = Pipe()

        proc.standardInput = inPipe
        proc.standardOutput = outPipe
        proc.standardError = errPipe

        do {
            try proc.run()
        } catch {
            throw TransportError.connectionFailed("Failed to spawn SSH process at \(configuration.sshBinaryPath): \(error.localizedDescription)")
        }

        self.process = proc
        self.stdinPipe = inPipe
        self.stdoutPipe = outPipe
        self.stderrPipe = errPipe
        self.isConnected = true

        startStderrReader(errPipe: errPipe)
        startStdoutReader(outPipe: outPipe, proc: proc)
    }

    public func send(data: Data) async throws {
        if !isConnected {
            try connect()
        }
        guard !isClosed, let inPipe = stdinPipe, let proc = process, proc.isRunning else {
            throw TransportError.closed
        }

        let fileHandle = inPipe.fileHandleForWriting
        do {
            try fileHandle.write(contentsOf: data)
        } catch {
            let stderrDiag = stderrAccumulator.summary()
            if !stderrDiag.isEmpty {
                throw TransportError.ioError("SSH write failed: \(error.localizedDescription). Stderr: \(stderrDiag)")
            } else {
                throw TransportError.ioError("SSH write failed: \(error.localizedDescription)")
            }
        }
    }

    public nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        Task { [weak self] in
            try? await self?.ensureConnected()
        }
        return stream
    }

    private func ensureConnected() throws {
        if !isConnected {
            try connect()
        }
    }

    private func startStderrReader(errPipe: Pipe) {
        let handle = errPipe.fileHandleForReading
        let accumulator = self.stderrAccumulator

        stderrTask = Task.detached {
            while !Task.isCancelled {
                let data = handle.availableData
                if data.isEmpty {
                    break
                }
                if let str = String(data: data, encoding: .utf8) {
                    accumulator.append(str)
                }
            }
        }
    }

    private func startStdoutReader(outPipe: Pipe, proc: Process) {
        let handle = outPipe.fileHandleForReading
        let cont = self.continuation
        let accumulator = self.stderrAccumulator

        proc.terminationHandler = { process in
            let status = process.terminationStatus
            if status != 0 {
                let stderrSummary = accumulator.summary()
                let errorMsg = stderrSummary.isEmpty
                    ? "SSH process terminated with exit code \(status)"
                    : "SSH process terminated with exit code \(status): \(stderrSummary)"
                cont.finish(throwing: TransportError.connectionFailed(errorMsg))
            } else {
                cont.finish()
            }
        }

        readTask = Task.detached {
            while !Task.isCancelled {
                let data = handle.availableData
                if data.isEmpty {
                    // EOF on stdout
                    break
                }
                cont.yield(data)
            }
        }
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true
        isConnected = false

        readTask?.cancel()
        readTask = nil
        stderrTask?.cancel()
        stderrTask = nil

        continuation.finish()

        try? stdinPipe?.fileHandleForWriting.close()
        stdinPipe = nil

        if let proc = process, proc.isRunning {
            proc.terminate()
        }
        process = nil
    }
}
