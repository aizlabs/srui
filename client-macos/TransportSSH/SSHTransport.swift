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

/// Ensures the receive-stream continuation is finished at most once across reader, close, and deinit paths.
private final class StreamFinishGuard: @unchecked Sendable {
    private let lock = NSLock()
    private var finished = false
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    init(_ continuation: AsyncThrowingStream<Data, Error>.Continuation) {
        self.continuation = continuation
    }

    func finish() {
        lock.lock()
        defer { lock.unlock() }
        guard !finished else { return }
        finished = true
        continuation.finish()
    }

    func finish(throwing error: Error) {
        lock.lock()
        defer { lock.unlock() }
        guard !finished else { return }
        finished = true
        continuation.finish(throwing: error)
    }
}

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
    private var stdinFD: Int32 = -1
    private var stdoutLatch = SocketReadLatch()
    private var stdoutReadThread: Thread?
    private var stderrReadThread: Thread?

    private var isClosed = false
    private var isConnected = false
    private let stderrAccumulator = StderrAccumulator()

    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation
    private let finishGuard: StreamFinishGuard

    public init(configuration: SSHConfiguration) {
        self.configuration = configuration
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation
        self.finishGuard = StreamFinishGuard(continuation)

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
        process?.terminationHandler = nil
        if let proc = process, proc.isRunning {
            proc.terminate()
        }
        stdoutLatch.stop()
        continuation.onTermination = nil
        finishGuard.finish()
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

        let stdoutFD = outPipe.fileHandleForReading.fileDescriptor
        let stderrFD = errPipe.fileHandleForReading.fileDescriptor
        let stdinWriteFD = inPipe.fileHandleForWriting.fileDescriptor

        self.process = proc
        self.stdinFD = stdinWriteFD
        self.isConnected = true

        startStderrReader(stderrFD: stderrFD)
        startStdoutReader(stdoutFD: stdoutFD, process: proc)
    }

    public func send(data: Data) async throws {
        if !isConnected {
            try connect()
        }
        guard !isClosed, stdinFD >= 0, let proc = process, proc.isRunning else {
            throw TransportError.closed
        }

        let fd = stdinFD
        try data.withUnsafeBytes { rawBuffer in
            guard let baseAddress = rawBuffer.baseAddress else { return }
            var bytesWritten = 0
            let totalBytes = rawBuffer.count

            while bytesWritten < totalBytes {
                let chunkPtr = baseAddress.advanced(by: bytesWritten)
                let remaining = totalBytes - bytesWritten
                let written = Darwin.write(fd, chunkPtr, remaining)

                if written < 0 {
                    let err = errno
                    if err == EINTR {
                        continue
                    }
                    let stderrDiag = stderrAccumulator.summary()
                    if !stderrDiag.isEmpty {
                        throw TransportError.ioError("SSH write failed: \(String(cString: strerror(err))). Stderr: \(stderrDiag)")
                    }
                    throw TransportError.ioError("SSH write failed: \(String(cString: strerror(err)))")
                } else if written == 0 {
                    throw TransportError.closed
                }

                bytesWritten += written
            }
        }
    }

    public nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        Task { [weak self] in
            guard let self else { return }
            do {
                try await self.ensureConnected()
            } catch {
                await self.finishAfterConnectFailure(error)
            }
        }
        return stream
    }

    private func finishAfterConnectFailure(_ error: Error) {
        finishGuard.finish(throwing: error)
    }

    private func ensureConnected() throws {
        if !isConnected {
            try connect()
        }
    }

    /// Runs stderr capture on a dedicated thread (§19.1: stderr separate from binary protocol).
    private func startStderrReader(stderrFD: Int32) {
        guard stderrReadThread == nil else { return }
        let accumulator = stderrAccumulator

        let thread = Thread {
            var buffer = [UInt8](repeating: 0, count: 65536)
            while true {
                let bytesRead = Darwin.read(stderrFD, &buffer, buffer.count)
                if bytesRead <= 0 {
                    break
                }
                if let str = String(bytes: buffer[0..<bytesRead], encoding: .utf8) {
                    accumulator.append(str)
                }
            }
        }
        thread.name = "org.srui.SSHTransport.stderr"
        thread.stackSize = 256 * 1024
        stderrReadThread = thread
        thread.start()
    }

    /// Runs stdout reads on a dedicated thread, draining to EOF before finishing the stream (§22.2).
    private func startStdoutReader(stdoutFD: Int32, process: Process) {
        guard stdoutReadThread == nil else { return }

        stdoutLatch.adopt(descriptor: stdoutFD)
        let cont = continuation
        let finishGuard = finishGuard
        let latch = stdoutLatch
        let accumulator = stderrAccumulator

        let thread = Thread {
            var buffer = [UInt8](repeating: 0, count: 65536)

            while true {
                guard let fd = latch.beginRead() else { break }
                let bytesRead = Darwin.read(fd, &buffer, buffer.count)
                latch.endRead()

                if bytesRead > 0 {
                    cont.yield(Data(buffer[0..<bytesRead]))
                } else if bytesRead == 0 {
                    break
                } else if errno == EINTR {
                    continue
                } else {
                    finishGuard.finish(throwing: TransportError.ioError("SSH read failed: \(String(cString: strerror(errno)))"))
                    return
                }
            }

            process.waitUntilExit()
            let status = process.terminationStatus
            if status != 0 {
                let stderrSummary = accumulator.summary()
                let errorMsg = stderrSummary.isEmpty
                    ? "SSH process terminated with exit code \(status)"
                    : "SSH process terminated with exit code \(status): \(stderrSummary)"
                finishGuard.finish(throwing: TransportError.connectionFailed(errorMsg))
            } else {
                finishGuard.finish()
            }
        }
        thread.name = "org.srui.SSHTransport.stdout"
        thread.stackSize = 512 * 1024
        stdoutReadThread = thread
        thread.start()
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true

        let wasConnected = isConnected
        isConnected = false

        process?.terminationHandler = nil
        stdoutLatch.stop()
        stdoutReadThread = nil
        stderrReadThread = nil

        if let proc = process, proc.isRunning {
            proc.terminate()
        }
        process = nil
        stdinFD = -1

        // When connected, the stdout reader drains to EOF and owns the single finish call.
        if !wasConnected {
            finishGuard.finish()
        }
    }
}
