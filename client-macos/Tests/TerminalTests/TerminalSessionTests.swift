//
// TerminalSessionTests.swift
// TerminalTests
//
// Offset accounting, VT parse, and island resync independence (§21, §21.2).
//

import Foundation
import SemanticModel
import Testing
@testable import Terminal

@Suite("TerminalSession")
struct TerminalSessionTests {
    private let stream = NodeId(7)

    @Test("equal offset advances and paints glyphs")
    func equalOffsetApplies() async throws {
        let session = TerminalSession()
        let snapshot = try await session.applyData(
            streamID: stream,
            byteOffset: 0,
            data: Data("hi".utf8)
        )
        #expect(snapshot.nextOffset == 2)
        #expect(snapshot.cells[0][0].character == "h")
        #expect(snapshot.cells[0][1].character == "i")
        #expect(await session.streamOffsets()[7] == 2)
    }

    @Test("older full duplicate is ignored")
    func fullDuplicateIgnored() async throws {
        let session = TerminalSession()
        _ = try await session.applyData(streamID: stream, byteOffset: 0, data: Data("ab".utf8))
        let after = try await session.applyData(streamID: stream, byteOffset: 0, data: Data("ab".utf8))
        #expect(after.nextOffset == 2)
        #expect(after.cells[0][0].character == "a")
        #expect(after.cells[0][2].character == " ")
    }

    @Test("partial overlap applies only the unseen suffix")
    func partialOverlap() async throws {
        let session = TerminalSession()
        _ = try await session.applyData(streamID: stream, byteOffset: 0, data: Data("ab".utf8))
        let after = try await session.applyData(streamID: stream, byteOffset: 1, data: Data("bcd".utf8))
        #expect(after.nextOffset == 4)
        #expect(after.plainText().hasPrefix("abcd") || after.cells[0][3].character == "d")
    }

    @Test("ahead of local cursor is a local resync")
    func aheadOffsetResets() async throws {
        let session = TerminalSession()
        _ = try await session.applyData(streamID: stream, byteOffset: 0, data: Data("ab".utf8))
        let after = try await session.applyData(streamID: stream, byteOffset: 10, data: Data("Z".utf8))
        #expect(after.nextOffset == 11)
        #expect(after.needsRedraw)
        #expect(after.cells[0][0].character == "Z")
    }

    @Test("TerminalResyncRequired clears the grid and jumps to resume_at")
    func resyncRequired() async throws {
        let session = TerminalSession()
        _ = try? await session.applyData(streamID: stream, byteOffset: 0, data: Data("hello".utf8))
        let after = try await session.applyResync(
            streamID: stream,
            requestedOffset: 0,
            retainedFromOffset: 8,
            resumeAtOffset: 16,
            cause: .retentionLoss
        )
        #expect(after.nextOffset == 16)
        #expect(after.needsRedraw)
        #expect(after.cells[0][0].character == " ")
        #expect(await session.streamOffsets()[7] == 16)
    }

    @Test("resync clears the screen but keeps remote input modes")
    func resyncPreservesInputModes() async throws {
        let session = TerminalSession()
        // DECSET 2004 (bracketed paste) + DECSET 1 (application cursor keys).
        var modes = Data()
        modes.append(contentsOf: [0x1B, 0x5B, 0x3F, 0x32, 0x30, 0x30, 0x34, 0x68])
        modes.append(contentsOf: [0x1B, 0x5B, 0x3F, 0x31, 0x68])
        _ = try await session.applyData(streamID: stream, byteOffset: 0, data: modes)
        #expect(await session.bracketedPaste(for: stream))
        #expect(await session.applicationCursorKeys(for: stream))

        _ = try await session.applyResync(
            streamID: stream,
            requestedOffset: 0,
            retainedFromOffset: 64,
            resumeAtOffset: 128,
            cause: .retentionLoss
        )
        #expect(await session.bracketedPaste(for: stream))
        #expect(await session.applicationCursorKeys(for: stream))

        // An ahead-of-cursor frame is a local resync too, and must not clear them either.
        _ = try await session.applyData(streamID: stream, byteOffset: 4096, data: Data("z".utf8))
        #expect(await session.bracketedPaste(for: stream))
        #expect(await session.applicationCursorKeys(for: stream))
    }

    @Test("replacement clears streams; same-session sync keeps live IDs")
    func replacementVersusSync() async throws {
        let session = TerminalSession()
        _ = try await session.applyData(streamID: stream, byteOffset: 0, data: Data("x".utf8))
        await session.syncPresentNodes([stream])
        #expect(await session.snapshot(for: stream) != nil)
        await session.syncPresentNodes([])
        #expect(await session.snapshot(for: stream) == nil)
        _ = try await session.applyData(streamID: stream, byteOffset: 0, data: Data("y".utf8))
        await session.resetForReplacementSession()
        #expect(await session.snapshot(for: stream) == nil)
    }

    @Test("empty and oversized frames are rejected")
    func frameLimits() async {
        let session = TerminalSession()
        await #expect(throws: TerminalApplyError.emptyFrame) {
            try await session.applyData(streamID: stream, byteOffset: 0, data: Data())
        }
        let huge = Data(repeating: 1, count: maxTerminalOutputFrameBytes + 1)
        await #expect(throws: TerminalApplyError.frameTooLarge(huge.count)) {
            try await session.applyData(streamID: stream, byteOffset: 0, data: huge)
        }
    }

    @Test("SGR, cursor, and erase do not invent widgets")
    func parserControls() async throws {
        let session = TerminalSession()
        var bytes = Data()
        bytes.append(contentsOf: "A".utf8)
        bytes.append(contentsOf: [0x1B, 0x5B, 0x32, 0x4A]) // CSI 2J
        bytes.append(contentsOf: [0x1B, 0x5B, 0x31, 0x3B, 0x31, 0x48]) // CUP 1;1
        bytes.append(contentsOf: "B".utf8)
        bytes.append(contentsOf: [0x1B, 0x5B, 0x33, 0x31, 0x6D]) // SGR red
        bytes.append(contentsOf: "C".utf8)
        let snapshot = try await session.applyData(streamID: stream, byteOffset: 0, data: bytes)
        #expect(snapshot.cells[0][0].character == "B")
        #expect(snapshot.cells[0][1].character == "C")
        #expect(snapshot.cells[0][1].attributes.foreground == .indexed(1))
    }

    @Test("OSC 52 is ignored and OSC 0 sets the title")
    func oscHandling() async throws {
        let session = TerminalSession()
        var bytes = Data()
        bytes.append(contentsOf: [0x1B, 0x5D, 0x30, 0x3B])
        bytes.append(contentsOf: "title-ok".utf8)
        bytes.append(0x07)
        bytes.append(contentsOf: [0x1B, 0x5D, 0x35, 0x32, 0x3B, 0x63, 0x3B])
        bytes.append(contentsOf: "secret".utf8)
        bytes.append(0x07)
        let snapshot = try await session.applyData(streamID: stream, byteOffset: 0, data: bytes)
        #expect(snapshot.title == "title-ok")
    }

    @Test("snapshots use bufferingNewest(1)")
    func snapshotStream() async throws {
        let session = TerminalSession()
        let updates = await session.snapshots(for: stream)
        _ = try await session.applyData(streamID: stream, byteOffset: 0, data: Data("1".utf8))
        _ = try await session.applyData(streamID: stream, byteOffset: 1, data: Data("2".utf8))
        var last: TerminalSnapshot?
        for await snapshot in updates {
            last = snapshot
            if snapshot.nextOffset >= 2 { break }
        }
        #expect(last?.nextOffset == 2)
    }
    @Test("OSC terminated by ESC backslash is handled")
    func oscStringTerminator() async throws {
        let session = TerminalSession()
        var bytes = Data()
        bytes.append(contentsOf: [0x1B, 0x5D, 0x30, 0x3B])
        bytes.append(contentsOf: "title-st".utf8)
        bytes.append(contentsOf: [0x1B, 0x5C]) // ESC \
        let snapshot = try await session.applyData(streamID: stream, byteOffset: 0, data: bytes)
        #expect(snapshot.title == "title-st")
    }

    @Test("charset designation ESC ( B is consumed without leaking into cells")
    func charsetDesignation() async throws {
        let session = TerminalSession()
        var bytes = Data([0x1B, 0x28, 0x42]) // ESC ( B
        bytes.append(contentsOf: "A".utf8)
        let snapshot = try await session.applyData(streamID: stream, byteOffset: 0, data: bytes)
        #expect(snapshot.cells[0][0].character == "A")
    }

    @Test("lines scrolling off the top are saved in scrollback")
    func scrollbackAccumulation() async throws {
        let session = TerminalSession()
        var text = ""
        for i in 0..<25 {
            text += "line\(i)\r\n"
        }
        let snapshot = try await session.applyData(streamID: stream, byteOffset: 0, data: Data(text.utf8))
        #expect(!snapshot.scrollback.isEmpty)
        let firstScrollbackLine = snapshot.scrollback[0].map { String($0.character) }.joined().trimmingCharacters(in: .whitespaces)
        #expect(firstScrollbackLine == "line0")
    }

    @Test("the stream table is bounded so a peer cannot name unbounded stream IDs")
    func streamTableIsBounded() async throws {
        let session = TerminalSession()
        for id in 0..<maxTerminalResumeMapEntries {
            _ = try await session.applyData(
                streamID: NodeId(UInt64(id)),
                byteOffset: 0,
                data: Data("x".utf8)
            )
        }
        // An existing stream still applies.
        _ = try await session.applyData(streamID: NodeId(0), byteOffset: 1, data: Data("y".utf8))
        await #expect(throws: TerminalApplyError.self) {
            try await session.applyData(
                streamID: NodeId(UInt64(maxTerminalResumeMapEntries)),
                byteOffset: 0,
                data: Data("x".utf8)
            )
        }
        await #expect(throws: TerminalApplyError.self) {
            try await session.applyResync(
                streamID: NodeId(UInt64(maxTerminalResumeMapEntries) + 1),
                requestedOffset: 0,
                retainedFromOffset: 0,
                resumeAtOffset: 8,
                cause: .retentionLoss
            )
        }
    }

    @Test("resync preserves bracketed paste mode")
    func resyncPreservesBracketedPaste() async throws {
        let session = TerminalSession()
        let enablePaste = Data([0x1B, 0x5B, 0x3F, 0x32, 0x30, 0x30, 0x34, 0x68])
        let snap1 = try await session.applyData(streamID: stream, byteOffset: 0, data: enablePaste)
        #expect(snap1.bracketedPaste)
        let snap2 = try await session.applyResync(
            streamID: stream,
            requestedOffset: 0,
            retainedFromOffset: 8,
            resumeAtOffset: 16,
            cause: .retentionLoss
        )
        #expect(snap2.bracketedPaste)
        #expect(snap2.needsRedraw)
        await session.acknowledgeRedraw(streamID: stream)
        #expect(await session.snapshot(for: stream)?.needsRedraw == false)
    }
}

@Suite("TerminalInputEncoder")
struct TerminalInputEncoderTests {
    @Test("special keys and bracketed paste")
    func encoder() {
        #expect(TerminalInputEncoder.encode(key: .enter) == Data([0x0D]))
        #expect(TerminalInputEncoder.encode(key: .arrowUp) == Data([0x1B, 0x5B, 0x41]))
        #expect(TerminalInputEncoder.encode(key: .arrowUp, applicationCursorKeys: true) == Data([0x1B, 0x4F, 0x41]))
        let paste = TerminalInputEncoder.encodePaste("hi", bracketed: true)
        #expect(paste.starts(with: [0x1B, 0x5B, 0x32, 0x30, 0x30, 0x7E]))
        #expect(paste.suffix(6) == Data([0x1B, 0x5B, 0x32, 0x30, 0x31, 0x7E]))
    }

    @Test("bracketed paste sanitizes closing sequence")
    func bracketedPasteSanitization() {
        let evil = "foo\u{1B}[201~malicious"
        let encoded = TerminalInputEncoder.encodePaste(evil, bracketed: true)
        let string = String(decoding: encoded, as: UTF8.self)
        #expect(string == "\u{1B}[200~foomalicious\u{1B}[201~")
    }
}
