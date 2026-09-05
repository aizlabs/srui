//
// TextEditingSessionTests.swift
// TextTests
//
// Per-node edit sequencing, debounce coalescing, remount retention, and
// authoritative echo/correction resolution (§18.3, §22.6).
//

import Foundation
import SemanticModel
import Testing
@testable import Text

@Suite("TextEditingSession")
@MainActor
struct TextEditingSessionTests {
    private let nodeID = NodeId(12)

    @Test("Quiet-period debounce coalesces to the newest whole value")
    func debounceCoalescesToNewestValue() async throws {
        let session = TextEditingSession(debounceNanoseconds: 40_000_000)
        var commits: [(String, EditSeq)] = []
        session.onCommit = { _, text, seq, _ in
            commits.append((text, seq))
        }

        session.noteLocalValue("a", nodeID: nodeID, composing: false, flushImmediately: false)
        session.noteLocalValue("ab", nodeID: nodeID, composing: false, flushImmediately: false)
        session.noteLocalValue("abc", nodeID: nodeID, composing: false, flushImmediately: false)

        // The debounce timer is a main-actor task competing with every other @MainActor suite
        // in the run, so wait for the commit to land rather than for one debounce period to
        // elapse: under a saturated actor the timer misses a fixed 80 ms budget and the test
        // reports "coalesced to zero commits".
        for _ in 0..<200 where commits.isEmpty {
            try await Task.sleep(nanoseconds: 10_000_000)
        }

        // `#require`, not a bare subscript. A missed commit must fail this test; `commits[0]`
        // on an empty array traps and takes the whole `swift test` process down with it.
        let commit = try #require(commits.first)
        #expect(commits.count == 1)
        #expect(commit.0 == "abc")
        #expect(commit.1.rawValue == 1)
        #expect(session.nextEditSeqValue(for: nodeID) == 2)
    }

    @Test("Each committed flush increments edit_seq without wrapping")
    func committedFlushesIncrementEditSeq() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var seqs: [UInt64] = []
        session.onCommit = { _, _, seq, _ in
            seqs.append(seq.rawValue)
        }
        session.noteLocalValue("one", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteLocalValue("two", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteLocalValue("three", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(seqs == [1, 2, 3])
        #expect(session.nextEditSeqValue(for: nodeID) == 4)
    }

    @Test("Same-session remounts keep sequence state; deletes drop it")
    func remountKeepsSequenceStateAndDeleteDropsIt() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        session.noteLocalValue("typed", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(session.nextEditSeqValue(for: nodeID) == 2)

        session.syncPresentNodes([nodeID, NodeId(99)])
        #expect(session.nextEditSeqValue(for: nodeID) == 2)

        session.syncPresentNodes([NodeId(99)])
        #expect(session.nextEditSeqValue(for: nodeID) == 1)
        #expect(session.localValue(for: nodeID) == nil)
    }

    @Test("Replacement reset clears both sequence spaces for the old incarnation")
    func replacementResetClearsNodeState() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        session.noteLocalValue("typed", nodeID: nodeID, composing: false, flushImmediately: true)
        session.resetForReplacementSession()
        #expect(session.nextEditSeqValue(for: nodeID) == 1)
        #expect(session.localValue(for: nodeID) == nil)
    }

    @Test("Accepted echo matching the submitted value keeps newer local typing")
    func acceptedEchoDoesNotOverwriteNewerTyping() throws {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        session.noteLocalValue("hello", nodeID: nodeID, composing: false, flushImmediately: true)
        let assigned = Event.textEdit(
            eventSeq: 1,
            eventId: EventId(string: "e1"),
            observedRevision: Revision(1),
            nodeId: nodeID,
            text: "hello",
            editSeq: try #require(EditSeq(1))
        )
        session.noteAssigned(assigned)
        session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: false)

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .keepLocal)
        #expect(session.localValue(for: nodeID) == "hello!")
        #expect(commits == ["hello"])
    }

    @Test("A different published value is a correction and invalidates drafts")
    func correctionReplacesNativeAndInvalidatesDrafts() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var invalidated: [NodeId] = []
        session.onInvalidateOutboxDraft = { id, _ in invalidated.append(id) }
        session.noteLocalValue("nope", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteAssigned(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "nope",
                editSeq: try #require(EditSeq(1))
            )
        )

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "corrected") == .apply)
        #expect(session.localValue(for: nodeID) == "corrected")
        #expect(invalidated == [nodeID])
    }

    @Test("Conflicting authoritative updates wait until marked text ends")
    func deferredAuthoritativeAppliesAfterComposition() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        _ = session.setComposing(true, nodeID: nodeID)
        session.noteLocalValue("composing", nodeID: nodeID, composing: true, flushImmediately: false)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "server") == .deferred)
        #expect(session.localValue(for: nodeID) == "composing")

        let applied = session.setComposing(false, nodeID: nodeID)
        #expect(applied == "server")
        #expect(session.localValue(for: nodeID) == "server")
    }

    @Test("Reapplying the last known store value keeps a local draft")
    func remountOfUnchangedValueKeepsLocalDraft() throws {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        var invalidated: [NodeId] = []
        var commits: [String] = []
        session.onInvalidateOutboxDraft = { id, _ in invalidated.append(id) }
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .apply)
        session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: false)

        let resolution = try session.withPreservedLocalText {
            session.applyPublishedValue(nodeID: nodeID, published: "hello")
        }
        #expect(resolution == .keepLocal)
        #expect(session.localValue(for: nodeID) == "hello!")
        #expect(invalidated.isEmpty)
        #expect(commits.isEmpty)
    }

    @Test("A remount while an assigned edit is in flight keeps local text")
    func remountWithAssignedEditKeepsLocal() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .apply)
        session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteAssigned(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "hello!",
                editSeq: try #require(EditSeq(1))
            )
        )

        let resolution = session.withPreservedLocalText {
            session.applyPublishedValue(nodeID: nodeID, published: "hello")
        }
        #expect(resolution == .keepLocal)
        #expect(session.localValue(for: nodeID) == "hello!")
    }

    @Test("A remount with no local draft reapplies the store string")
    func remountWithoutDraftApplies() {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .apply)
        let resolution = session.withPreservedLocalText {
            session.applyPublishedValue(nodeID: nodeID, published: "hello")
        }
        #expect(resolution == .apply)
        #expect(session.localValue(for: nodeID) == "hello")
    }

    @Test("A remount after a non-publishing acknowledgement applies the store string")
    func remountAfterAcknowledgedRejectApplies() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "bar") == .apply)
        session.noteLocalValue("foo", nodeID: nodeID, composing: false, flushImmediately: true)
        let assigned = Event.textEdit(
            eventSeq: 1,
            eventId: EventId(string: "e1"),
            observedRevision: Revision(1),
            nodeId: nodeID,
            text: "foo",
            editSeq: try #require(EditSeq(1))
        )
        session.noteAssigned(assigned)
        session.noteAcknowledged(assigned)

        let resolution = session.withPreservedLocalText {
            session.applyPublishedValue(nodeID: nodeID, published: "bar")
        }
        #expect(resolution == .apply)
        #expect(session.localValue(for: nodeID) == "bar")
    }

    @Test("Cancel matching the assigned event drops in-flight identity")
    func noteCanceledMatchingAssignedClearsSubmit() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "abc") == .apply)
        session.noteLocalValue("abcd", nodeID: nodeID, composing: false, flushImmediately: true)
        let assigned = Event.textEdit(
            eventSeq: 1,
            eventId: EventId(string: "e1"),
            observedRevision: Revision(1),
            nodeId: nodeID,
            text: "abcd",
            editSeq: try #require(EditSeq(1))
        )
        session.noteAssigned(assigned)
        session.noteCanceled(nodeID: nodeID, eventId: assigned.eventId)

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "abc") == .apply)
        #expect(session.localValue(for: nodeID) == "abc")
    }

    @Test("Cancel for a different event id leaves the assigned edit in place")
    func noteCanceledMismatchLeavesAssigned() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "abc") == .apply)
        session.noteLocalValue("abcd", nodeID: nodeID, composing: false, flushImmediately: true)
        let assigned = Event.textEdit(
            eventSeq: 1,
            eventId: EventId(string: "e1"),
            observedRevision: Revision(1),
            nodeId: nodeID,
            text: "abcd",
            editSeq: try #require(EditSeq(1))
        )
        session.noteAssigned(assigned)
        session.noteCanceled(nodeID: nodeID, eventId: EventId(string: "other"))

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "abcd") == .keepLocal)
        #expect(session.localValue(for: nodeID) == "abcd")
    }

    @Test("A rejection that republishes the previous value replaces local text")
    func rejectionRevertingToLastKnownApplies() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var invalidated: [NodeId] = []
        session.onInvalidateOutboxDraft = { id, _ in invalidated.append(id) }

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "abc") == .apply)
        session.noteLocalValue("abcd", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteAssigned(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "abcd",
                editSeq: try #require(EditSeq(1))
            )
        )

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "abc") == .apply)
        #expect(session.localValue(for: nodeID) == "abc")
        #expect(invalidated == [nodeID])
    }

    @Test("Composition end does not flush the previous marked string")
    func compositionEndDoesNotFlushMarkedValue() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        _ = session.setComposing(true, nodeID: nodeID)
        session.noteLocalValue("hel", nodeID: nodeID, composing: true, flushImmediately: false)
        #expect(session.setComposing(false, nodeID: nodeID) == nil)
        #expect(commits.isEmpty)

        session.noteLocalValue("hello", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(commits == ["hello"])
    }

    @Test("Marked text does not emit an edit")
    func composingSuppressesRemoteEmission() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var commits = 0
        session.onCommit = { _, _, _, _ in commits += 1 }
        session.noteLocalValue("á", nodeID: nodeID, composing: true, flushImmediately: true)
        #expect(commits == 0)
        session.endEditing(nodeID: nodeID)
        #expect(commits == 0)
    }
}
