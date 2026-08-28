//
// TCPSocketTransport.swift
// TransportSSH
//
// POSIX TCP stream socket transport adapter (§20.2, §22).
//

import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// Transport adapter actor communicating over a TCP stream socket (e.g. `127.0.0.1:port`, §20.2).
public actor TCPSocketTransport: Transport {
    public let host: String
    public let port: UInt16

    private var socketFD: Int32 = -1
    private var isClosed = false
    private var readTask: Task<Void, Never>?
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    public init(host: String = "127.0.0.1", port: UInt16) {
        self.host = host
        self.port = port
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

    /// Establishes the TCP socket connection to the server.
    public func connect() throws {
        guard socketFD == -1 else {
            return
        }

        let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw TransportError.connectionFailed("Failed to create TCP socket: \(String(cString: strerror(errno)))")
        }

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = port.bigEndian

        let inetResult = inet_pton(AF_INET, host, &addr.sin_addr)
        guard inetResult == 1 else {
            Darwin.close(fd)
            throw TransportError.connectionFailed("Invalid IP address: \(host)")
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_in>.size)
        let connectResult = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                Darwin.connect(fd, saPtr, addrLen)
            }
        }

        guard connectResult == 0 else {
            let err = errno
            Darwin.close(fd)
            throw TransportError.connectionFailed("Failed to connect to \(host):\(port): \(String(cString: strerror(err)))")
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
                    throw TransportError.ioError("TCP write failed: \(String(cString: strerror(err)))")
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
                        cont.finish(throwing: TransportError.ioError("TCP read failed: \(String(cString: strerror(err)))"))
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
