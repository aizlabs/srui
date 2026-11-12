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

/// The SSH child's exit status, recorded by its termination handler, so a failed write can report
/// why the child went away rather than the `EPIPE` its exit caused (§19.1, §25, §4 inv. 13).
private final class ChildExitRecord: @unchecked Sendable {
    private let lock = NSLock()
    private var status: Int32?
    private var waiters: [UUID: CheckedContinuation<Int32?, Never>] = [:]

    /// The recorded status, without waiting.
    var recorded: Int32? {
        lock.lock()
        defer { lock.unlock() }
        return status
    }

    /// Records the exit; the first record wins (the termination handler and the stdout reader
    /// both report the same exit).
    func record(_ exitStatus: Int32) {
        lock.lock()
        if status == nil {
            status = exitStatus
        }
        let exitStatus = status ?? exitStatus
        let parked = waiters
        waiters.removeAll()
        lock.unlock()
        for waiter in parked.values {
            waiter.resume(returning: exitStatus)
        }
    }

    /// The exit status, waiting at most `timeout` for the child to exit; `nil` if it has not.
    func status(waitingAtMost timeout: Duration) async -> Int32? {
        let id = UUID()
        return await withCheckedContinuation { continuation in
            lock.lock()
            if let status {
                lock.unlock()
                continuation.resume(returning: status)
                return
            }
            waiters[id] = continuation
            lock.unlock()
            Task {
                try? await Task.sleep(for: timeout)
                self.expire(id)
            }
        }
    }

    private func expire(_ id: UUID) {
        lock.lock()
        let waiter = waiters.removeValue(forKey: id)
        lock.unlock()
        waiter?.resume(returning: nil)
    }
}

/// Actor implementing the SRUI secure SSH transport binding over an OpenSSH subsystem channel (§19, §19.1, §25).
public actor SSHTransport: Transport {
    public let configuration: SSHConfiguration

    private var process: Process?
    /// Latched duplicate of the SSH child's stdin. The `Pipe` keeps owning the original; this
    /// process-local copy is what the writer queue claims, so releasing the `Pipe` in `close()`
    /// can never recycle a descriptor number a parked `write(2)` is still holding.
    private var stdinLatch = SocketReadLatch()
    private var hasStdin = false
    /// Latched duplicate of the child's stdout, for the same reason as `stdinLatch`: the `Pipe`
    /// closes its own descriptor when `process` is released, so the reader must hold a private
    /// copy or `close()` can free a descriptor number out from under an in-flight `read(2)`.
    private var stdoutLatch = SocketReadLatch()
    /// Latched duplicate of the child's stderr. Latched rather than raw so `close()` can stop the
    /// diagnostic reader instead of leaving a thread parked in `read(2)` for the process lifetime.
    private var stderrLatch = SocketReadLatch()
    private var stdoutReadThread: Thread?
    private var stderrReadThread: Thread?
    /// Writes run off the actor so `close()` can preempt a stalled SSH child (§22.2).
    private let writer = SocketWriter(label: "org.srui.SSHTransport.write")
    /// Bounds inbound read-ahead so a stalled consumer cannot grow memory without limit (§26).
    private let backlogGate = InboundBacklogGate()

    private var isClosed = false
    private var isConnected = false
    private let stderrAccumulator = StderrAccumulator()
    private let childExit = ChildExitRecord()
    /// The child's exit failure when the transport closed because the child had already failed
    /// (the stdout reader finishes the stream, whose termination closes the transport). A later
    /// `send` reports it instead of a bare `.closed`; a close made while the child ran keeps `nil`.
    private var exitFailureAtClose: TransportError?

    /// How long a failed write waits for the child's exit status before reporting the raw I/O
    /// error. `ssh` closes its stdin by exiting, and the `EPIPE` reaches the writer before the
    /// child is reaped, so without this wait a host-key rejection surfaced as either
    /// `.connectionFailed` or a bare broken pipe depending on scheduling. Bounded, so a child that
    /// closed its stdin and keeps running still yields the I/O error.
    static let defaultChildExitClassificationBound: Duration = .seconds(5)
    let childExitClassificationBound: Duration

    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation
    private let finishGuard: StreamFinishGuard

    public init(configuration: SSHConfiguration) {
        self.init(
            configuration: configuration,
            childExitClassificationBound: Self.defaultChildExitClassificationBound
        )
    }

    /// `childExitClassificationBound` is a parameter only so a test of the bound need not hold a
    /// child's pipes open for the production five seconds.
    init(configuration: SSHConfiguration, childExitClassificationBound: Duration) {
        self.configuration = configuration
        self.childExitClassificationBound = childExitClassificationBound
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
        writer.stop()
        stdinLatch.stop()
        stdoutLatch.stop()
        // Stopped here as well as in `close()`: a latch never marked stopped never performs its
        // deferred close, so a transport released without an explicit `close()` would leak its
        // private duplicate of the child's stderr (§22.2).
        stderrLatch.stop()
        backlogGate.release()
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
        let childExit = self.childExit
        proc.terminationHandler = { finished in
            childExit.record(finished.terminationStatus)
        }

        do {
            try proc.run()
        } catch {
            throw TransportError.connectionFailed("Failed to spawn SSH process at \(configuration.sshBinaryPath): \(error.localizedDescription)")
        }

        // Each parent-side pipe end is duplicated, then the `Pipe`'s own handle is closed, so the
        // latch that adopts the duplicate becomes the descriptor's sole owner.
        //
        // Sharing a descriptor with the `Pipe` gives it two owners: the latch closes it in
        // `close()` and the `FileHandle` closes it again when `process` is released. The second
        // close lands on whatever the kernel has since handed that number to — an unrelated
        // subsystem's socket, or worse, a descriptor the reader thread is still inside `read(2)`
        // on. Duplicating also decouples teardown from `Pipe` deallocation timing, which nothing
        // here controls (§22.2).
        let handles = [
            inPipe.fileHandleForWriting,
            outPipe.fileHandleForReading,
            errPipe.fileHandleForReading,
        ]
        let descriptors = handles.map { Darwin.dup($0.fileDescriptor) }

        guard descriptors.allSatisfy({ $0 >= 0 }) else {
            let failure = String(cString: strerror(errno))
            for descriptor in descriptors where descriptor >= 0 {
                Darwin.close(descriptor)
            }
            proc.terminate()
            throw TransportError.connectionFailed("Failed to duplicate SSH pipes: \(failure)")
        }

        // Safe once the duplicates exist: the child holds its own ends, so neither the stdin write
        // end nor the stdout/stderr read ends disappear from underneath it.
        for handle in handles {
            try? handle.close()
        }

        let stdinWriteFD = descriptors[0]
        let stdoutFD = descriptors[1]
        let stderrFD = descriptors[2]

        self.process = proc
        self.stdinLatch.adopt(descriptor: stdinWriteFD)
        self.hasStdin = true
        self.isConnected = true

        startStderrReader(stderrFD: stderrFD)
        startStdoutReader(stdoutFD: stdoutFD, process: proc)
    }

    public func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        if isClosed {
            throw exitFailureAtClose ?? TransportError.closed
        }
        if !isConnected {
            try connect()
        }
        guard hasStdin, let proc = process else {
            throw TransportError.closed
        }
        guard proc.isRunning else {
            // The child already exited: report its exit as the stdout reader does.
            throw Self.exitFailure(status: proc.terminationStatus, stderr: stderrAccumulator.summary())
                ?? TransportError.closed
        }

        // Off the actor executor: a blocking write to a child that stopped reading would hold this
        // actor and make the actor-isolated `close()` unreachable (§22.2).
        do {
            try await writer.write(data, logicalClass: logicalClass, claiming: stdinLatch)
        } catch let error as TransportError {
            // A write fails with `EPIPE` once the child has closed its stdin, which `ssh` does by
            // exiting. Classify by that exit (bounded wait), not by the order the two were seen.
            if case .ioError = error, !isClosed,
               let status = await childExit.status(waitingAtMost: childExitClassificationBound),
               let failure = Self.exitFailure(status: status, stderr: stderrAccumulator.summary()) {
                throw failure
            }
            let stderrDiag = stderrAccumulator.summary()
            guard !stderrDiag.isEmpty else { throw error }
            throw TransportError.ioError("\(error.description). Stderr: \(stderrDiag)")
        }
    }

    public func acknowledgeReceived(byteCount: Int) async {
        backlogGate.recordConsumed(byteCount)
    }

    /// Bytes read from the SSH channel that the consumer has not acknowledged yet (§26).
    public var pendingInboundBytes: Int {
        backlogGate.outstandingBytes
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
    ///
    /// Latched and poll-driven like the stdout reader so `close()` terminates it. A raw blocking
    /// `read(2)` here would leave the thread parked for the lifetime of the process, holding a
    /// descriptor nothing can revoke (§22.2).
    private func startStderrReader(stderrFD: Int32) {
        guard stderrReadThread == nil else { return }
        stderrLatch.adopt(descriptor: stderrFD)
        let accumulator = stderrAccumulator
        let latch = stderrLatch

        let thread = Thread {
            var buffer = [UInt8](repeating: 0, count: 65536)
            readLoop: while true {
                switch readAvailable(from: latch, into: &buffer) {
                case .bytes(let count):
                    if let str = String(bytes: buffer[0..<count], encoding: .utf8) {
                        accumulator.append(str)
                    }
                case .retry:
                    continue readLoop
                case .stopped, .endOfStream, .failed:
                    break readLoop
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
        let gate = backlogGate
        let childExit = childExit

        let thread = Thread {
            var buffer = [UInt8](repeating: 0, count: 65536)

            readLoop: while true {
                // §26: stop draining the channel while the consumer is behind, rather than
                // buffering committed transactions without bound.
                guard gate.waitForCapacity() else { break readLoop }

                switch readAvailable(from: latch, into: &buffer) {
                case .bytes(let count):
                    gate.recordDelivered(count)
                    cont.yield(Data(buffer[0..<count]))
                case .retry:
                    continue readLoop
                case .stopped, .endOfStream:
                    break readLoop
                case .failed(let err):
                    finishGuard.finish(throwing: TransportError.ioError(
                        "SSH read failed: \(String(cString: strerror(err)))"
                    ))
                    return
                }
            }

            process.waitUntilExit()
            // Recorded before the stream finishes, so the close its termination triggers sees it.
            childExit.record(process.terminationStatus)
            if let failure = SSHTransport.exitFailure(
                status: process.terminationStatus,
                stderr: accumulator.summary()
            ) {
                finishGuard.finish(throwing: failure)
            } else {
                finishGuard.finish()
            }
        }
        thread.name = "org.srui.SSHTransport.stdout"
        thread.stackSize = 512 * 1024
        stdoutReadThread = thread
        thread.start()
    }

    /// The one classification of a child exit, shared by the stdout reader and `send`: a non-zero
    /// status is a connection failure carrying the child's stderr; a clean exit is not a failure.
    private static func exitFailure(status: Int32, stderr: String) -> TransportError? {
        guard status != 0 else { return nil }
        return .connectionFailed(stderr.isEmpty
            ? "SSH process terminated with exit code \(status)"
            : "SSH process terminated with exit code \(status): \(stderr)")
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true
        exitFailureAtClose = childExit.recorded.flatMap {
            Self.exitFailure(status: $0, stderr: stderrAccumulator.summary())
        }

        let wasConnected = isConnected
        isConnected = false

        process?.terminationHandler = nil
        // Stopping the writer and its latch first unblocks any write parked against a child that
        // stopped reading, so teardown never waits on a stalled peer (§22.2).
        writer.stop()
        stdinLatch.stop()
        stdoutLatch.stop()
        stderrLatch.stop()
        backlogGate.release()
        stdoutReadThread = nil
        stderrReadThread = nil

        if let proc = process, proc.isRunning {
            proc.terminate()
        }
        process = nil
        hasStdin = false

        // When connected, the stdout reader drains to EOF and owns the single finish call.
        if !wasConnected {
            finishGuard.finish()
        }
    }
}
