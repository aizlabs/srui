//
// TerminalCommandPumpTests.swift
// SRUITests
//
// Terminal input framing for oversized clipboard pastes (§21, §26).
//

import Testing
import Foundation
import Protocol
import SemanticModel
import Terminal
@testable import Session
@testable import TransportSSH

private actor PasteRecordingTransport: Transport {
    private(set) var frames: [Data] = []
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        frames.append(data)
    }

    func recordedFrames() -> [Data] { frames }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> { stream }

    func close() async {
        continuation.finish()
    }
}

@Suite("TerminalCommandPump paste framing")
struct TerminalCommandPumpTests {
    private let stream = NodeId(7)

    /// A paste just over the per-envelope limit used to be discarded outright, so the user got
    /// no terminal input at all. It must be split into ordered frames instead (§21, §26).
    @Test("An oversized paste is chunked into ordered TerminalInput frames")
    func oversizedPasteIsChunkedNotDropped() async throws {
        let transport = PasteRecordingTransport()
        let pump = TerminalCommandPump()
        await pump.attach(transport: transport)

        let text = String(repeating: "a", count: maxTerminalInputBytes + 100)
        let payload = TerminalInputEncoder.encodePaste(text, bracketed: true)
        #expect(payload.count > maxTerminalInputBytes)
        await pump.enqueueInput(streamID: stream, data: payload)

        try await AsyncTestSupport.eventuallyAsync(description: "paste frames written") {
            await transport.recordedFrames().count == 2
        }

        var reassembled = Data()
        for framed in await transport.recordedFrames() {
            let message = try SRUIFraming.decodeFramed(SRUIMessage.self, from: framed)
            guard case .terminalInput(let input)? = message.msg else {
                Issue.record("expected TerminalInput, got \(String(describing: message.msg))")
                return
            }
            #expect(input.streamID == stream.value)
            #expect(!input.data.isEmpty)
            #expect(input.data.count <= maxTerminalInputBytes)
            reassembled.append(input.data)
        }
        #expect(reassembled == payload)
        // The single bracketed-paste pair survives the split.
        #expect(reassembled.starts(with: Data([0x1B, 0x5B, 0x32, 0x30, 0x30, 0x7E])))
        #expect(reassembled.suffix(6) == Data([0x1B, 0x5B, 0x32, 0x30, 0x31, 0x7E]))

        await pump.disconnect()
        await transport.close()
    }

    /// Chunk boundaries must not cut a multi-byte scalar in half.
    @Test("Chunk boundaries fall on UTF-8 scalar boundaries")
    func chunkBoundariesRespectUTF8() async throws {
        let transport = PasteRecordingTransport()
        let pump = TerminalCommandPump()
        await pump.attach(transport: transport)

        // "é" is two bytes and the leading "x" makes every scalar start at an odd offset, so a
        // naive cut at exactly maxTerminalInputBytes would land on a continuation byte.
        let text = "x" + String(repeating: "é", count: maxTerminalInputBytes)
        let payload = Data(text.utf8)
        #expect(payload.count == maxTerminalInputBytes * 2 + 1)
        await pump.enqueueInput(streamID: stream, data: payload)

        try await AsyncTestSupport.eventuallyAsync(description: "utf8 paste frames written") {
            await transport.recordedFrames().count == 3
        }

        var reassembled = Data()
        for framed in await transport.recordedFrames() {
            let message = try SRUIFraming.decodeFramed(SRUIMessage.self, from: framed)
            guard case .terminalInput(let input)? = message.msg else {
                Issue.record("expected TerminalInput, got \(String(describing: message.msg))")
                return
            }
            #expect(input.data.count <= maxTerminalInputBytes)
            #expect(String(data: input.data, encoding: .utf8) != nil)
            reassembled.append(input.data)
        }
        #expect(reassembled == payload)

        await pump.disconnect()
        await transport.close()
    }
}
