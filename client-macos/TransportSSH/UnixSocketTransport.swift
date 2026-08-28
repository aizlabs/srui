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

/// Transport adapter actor communicating with a local Unix domain socket daemon (`srui-sessiond`, §20.2).
public actor UnixSocketTransport: Transport {
    public let socketPath: String

    private var socketFD: Int32 = -1
    private var isClosed = false
    private var readTask: Task<Void, Never>?
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
        let fd = socketFD
        if fd >= 0 {
            Darwin.shutdown(fd, SHUT_RDWR)
            Darwin.close(fd)
        }
    }

    /// Establishes the Unix domain socket connection to the server.
    public func connect() throws {
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
        self.isClosed = false
        startReadingLoop()
    }

    public func send(data: Data) async throws {
        if socketFD == -1 {
            try connect()
        }
        guard !isClosed, socketFD >= 0 else {
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
        if socketFD == -1 {
            try connect()
        }
    }

    private func startReadingLoop() {
        guard readTask == nil else { return }
        let fd = self.socketFD
        let cont = self.continuation

        let task = Task.detached {
            var buffer = [UInt8](repeating: 0, count: 65536)

            while !Task.isCancelled {
                let bytesRead = Darwin.read(fd, &buffer, buffer.count)

                if bytesRead > 0 {
                    let chunk = Data(buffer[0..<bytesRead])
                    cont.yield(chunk)
                } else if bytesRead == 0 {
                    // EOF
                    cont.finish()
                    break
                } else {
                    let err = errno
                    if err == EINTR {
                        continue
                    }
                    if err == EBADF || err == ECONNRESET || err == ENOTCONN {
                        cont.finish()
                    } else {
                        cont.finish(throwing: TransportError.ioError("Socket read failed: \(String(cString: strerror(err)))"))
                    }
                    break
                }
            }
        }

        self.readTask = task
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true

        readTask?.cancel()
        readTask = nil
        continuation.finish()

        if socketFD >= 0 {
            Darwin.shutdown(socketFD, SHUT_RDWR)
            Darwin.close(socketFD)
            socketFD = -1
        }
    }
}
