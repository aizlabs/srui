//
// UnixSocketTransport.swift
// TransportSSH
//
// POSIX Unix Domain Socket transport adapter for local sessiond connections (§20.1, §20.2, §22).
//

import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// Owns the socket descriptor read by a blocking reader thread, and closes it exactly once.
///
/// A stop flag alone is not enough: `while !isStopped { read(fd) }` is check-then-act, so the
/// reader can pass the check, be descheduled while `close()` runs to completion, and then call
/// `read` on a descriptor number the kernel has already recycled to an unrelated `open` elsewhere
/// in the process — silently consuming another owner's bytes. The latch therefore keeps the
/// descriptor: a stop requested while the reader is inside `read(2)` only shuts the socket down (to
/// wake it) and defers the `close(2)` to the reader on its way out, so the number can never be
/// recycled while the reader might still touch it.
final class SocketReadLatch: @unchecked Sendable {
    private let lock = NSLock()
    private var _isStopped = false
    private var fd: Int32 = -1
    private var readerHoldsFD = false

    /// Transfers ownership of `descriptor` to the latch. After this only the latch closes it.
    func adopt(descriptor: Int32) {
        lock.lock()
        fd = descriptor
        lock.unlock()
    }

    /// Returns the descriptor to read from, or `nil` once stopped, claiming it for the reader.
    func beginRead() -> Int32? {
        lock.lock()
        defer { lock.unlock() }
        guard !_isStopped, fd >= 0 else { return nil }
        readerHoldsFD = true
        return fd
    }

    /// Releases the reader's claim, performing the deferred close if a stop landed mid-read.
    func endRead() {
        lock.lock()
        readerHoldsFD = false
        let doomed = _isStopped ? fd : -1
        if doomed >= 0 { fd = -1 }
        lock.unlock()

        if doomed >= 0 {
            Darwin.close(doomed)
        }
    }

    /// Stops the loop, wakes a blocked `read(2)`, and releases the descriptor unless the reader is
    /// currently inside `read`, in which case `endRead()` closes it.
    func stop() {
        lock.lock()
        _isStopped = true
        let current = fd
        let readerBusy = readerHoldsFD
        if current >= 0 && !readerBusy { fd = -1 }
        lock.unlock()

        guard current >= 0 else { return }
        Darwin.shutdown(current, SHUT_RDWR)
        if !readerBusy {
            Darwin.close(current)
        }
    }
}

/// Transport adapter actor communicating with a local Unix domain socket daemon (`srui-sessiond`, §20.2).
public actor UnixSocketTransport: Transport {
    public let socketPath: String

    private var socketFD: Int32 = -1
    private var isClosed = false
    private var readThread: Thread?
    private var readLatch = SocketReadLatch()
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    public init(socketPath: String) {
        self.socketPath = socketPath
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation

        continuation.onTermination = { [weak self] _ in
            Task {
                await self?.close()
            }
        }
    }

    deinit {
        // The latch owns the descriptor and closes it exactly once, deferring to the reader thread
        // if one is parked in `read(2)`.
        readLatch.stop()
        // Otherwise a consumer still iterating `receiveStream()` would hang forever. Drop the
        // termination handler first: it captures `self` weakly, and forming that reference while
        // the actor is mid-deallocation traps.
        continuation.onTermination = nil
        continuation.finish()
    }

    /// Establishes the Unix domain socket connection to the server.
    public func connect() throws {
        // A closed transport stays closed: its stream continuation is already finished, so silently
        // reconnecting here would open a socket whose every inbound byte is discarded.
        guard !isClosed else {
            throw TransportError.closed
        }
        guard socketFD == -1 else {
            return
        }

        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw TransportError.connectionFailed("Failed to create socket: \(String(cString: strerror(errno)))")
        }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)

        let pathBytes = socketPath.utf8CString
        let maxPathLen = MemoryLayout.size(ofValue: addr.sun_path)

        guard pathBytes.count <= maxPathLen else {
            Darwin.close(fd)
            throw TransportError.connectionFailed("Socket path length (\(pathBytes.count) bytes) exceeds maximum allowable length (\(maxPathLen) bytes)")
        }

        withUnsafeMutablePointer(to: &addr.sun_path) { sunPathPtr in
            let rawPtr = UnsafeMutableRawPointer(sunPathPtr)
            pathBytes.withUnsafeBytes { srcBytes in
                if let base = srcBytes.baseAddress {
                    rawPtr.copyMemory(from: base, byteCount: srcBytes.count)
                }
            }
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let connectResult = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                Darwin.connect(fd, saPtr, addrLen)
            }
        }

        guard connectResult == 0 else {
            let err = errno
            Darwin.close(fd)
            throw TransportError.connectionFailed("Failed to connect to \(socketPath): \(String(cString: strerror(err)))")
        }

        self.socketFD = fd
        readLatch.adopt(descriptor: fd)
        startReadingLoop()
    }

    public func send(data: Data) async throws {
        guard !isClosed else {
            throw TransportError.closed
        }
        if socketFD == -1 {
            try connect()
        }
        guard socketFD >= 0 else {
            throw TransportError.closed
        }
        let fd = socketFD

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
                    throw TransportError.ioError("Socket write failed: \(String(cString: strerror(err)))")
                } else if written == 0 {
                    throw TransportError.closed
                }

                bytesWritten += written
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
        guard !isClosed else {
            throw TransportError.closed
        }
        if socketFD == -1 {
            try connect()
        }
    }

    /// Runs the blocking `read(2)` loop on a dedicated thread.
    ///
    /// This deliberately does not use `Task.detached`: a task parked in a blocking syscall occupies
    /// one of the Swift cooperative pool's threads (sized to the core count) for the whole lifetime
    /// of the connection, starving the same pool that decodes and applies transactions (§22.2).
    private func startReadingLoop() {
        guard readThread == nil else { return }
        let cont = self.continuation
        let latch = self.readLatch

        let thread = Thread {
            var buffer = [UInt8](repeating: 0, count: 65536)

            while true {
                // The latch hands out the descriptor only while it is guaranteed open, and takes it
                // back in `endRead()`, so the number can never be recycled underneath this `read`.
                guard let fd = latch.beginRead() else { break }
                let bytesRead = Darwin.read(fd, &buffer, buffer.count)
                let err = errno
                latch.endRead()

                if bytesRead > 0 {
                    cont.yield(Data(buffer[0..<bytesRead]))
                } else if bytesRead == 0 {
                    // EOF
                    cont.finish()
                    return
                } else {
                    if err == EINTR {
                        continue
                    }
                    if err == EBADF || err == ECONNRESET || err == ENOTCONN {
                        cont.finish()
                    } else {
                        cont.finish(throwing: TransportError.ioError("Socket read failed: \(String(cString: strerror(err)))"))
                    }
                    return
                }
            }
            cont.finish()
        }
        thread.name = "org.srui.UnixSocketTransport.read"
        thread.stackSize = 512 * 1024
        self.readThread = thread
        thread.start()
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true

        // The latch stops the loop, shuts the socket down to wake a blocked `read`, and closes the
        // descriptor — or hands that close to the reader if it is inside `read(2)` right now. That
        // ordering is what makes joining the reader thread unnecessary: the number is never
        // released while the reader might still use it. `send` and `close` are both actor-isolated,
        // so no write can be in flight against this fd here.
        readLatch.stop()
        readThread = nil
        socketFD = -1
        continuation.finish()
    }
}
