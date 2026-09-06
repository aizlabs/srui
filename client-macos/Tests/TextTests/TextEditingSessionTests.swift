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

    @Test("A different published value is a correction and clears unassigned local work")
    func correctionReplacesNativeAndClearsUnassignedEdit() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
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
        session.noteLocalValue("nope!", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(session.hasUnsentSuccessorDraft(for: nodeID))

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "corrected") == .apply)
        #expect(session.localValue(for: nodeID) == "corrected")
        #expect(session.hasUnsentSuccessorDraft(for: nodeID) == false)
        #expect(session.nextUnassignedEditNode() == nil)
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
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .apply)
        session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: false)

        let resolution = session.withPreservedLocalText {
            session.applyPublishedValue(nodeID: nodeID, published: "hello")
        }
        #expect(resolution == .keepLocal)
        #expect(session.localValue(for: nodeID) == "hello!")
        #expect(session.hasUnsentSuccessorDraft(for: nodeID))
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

    @Test("End-editing inside a remount preserve scope does not flush the draft")
    func remountPreserveDoesNotFlushPendingDraft() {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .apply)
        session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: false)

        let resolution = session.withPreservedLocalText {
            session.endEditing(nodeID: nodeID)
            session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: true)
            return session.applyPublishedValue(nodeID: nodeID, published: "hello")
        }
        #expect(resolution == .keepLocal)
        #expect(session.localValue(for: nodeID) == "hello!")
        #expect(commits.isEmpty)
    }

    @Test("A remount while composing applies the store string instead of deferring it")
    func remountAbandonsCompositionSoStoreStringIsNotDeferred() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .apply)
        _ = session.setComposing(true, nodeID: nodeID)
        session.noteLocalValue("hel", nodeID: nodeID, composing: true, flushImmediately: false)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .deferred)
        #expect(session.isComposing(for: nodeID))

        let resolution = session.withPreservedLocalText {
            session.endEditing(nodeID: nodeID)
            session.noteLocalValue("hel", nodeID: nodeID, composing: true, flushImmediately: true)
            return session.applyPublishedValue(nodeID: nodeID, published: "hello")
        }
        #expect(resolution == .apply)
        #expect(!session.isComposing(for: nodeID))
        #expect(session.localValue(for: nodeID) == "hello")
        #expect(commits.isEmpty)

        session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(session.localValue(for: nodeID) == "hello!")
        #expect(commits == ["hello!"])
    }

    @Test("A remount while composing still keeps an unflushed committed draft")
    func remountAbandonsCompositionButKeepsPendingDraft() {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "hello") == .apply)
        session.noteLocalValue("hello!", nodeID: nodeID, composing: false, flushImmediately: false)
        _ = session.setComposing(true, nodeID: nodeID)
        session.noteLocalValue("hel", nodeID: nodeID, composing: true, flushImmediately: false)

        let resolution = session.withPreservedLocalText {
            return session.applyPublishedValue(nodeID: nodeID, published: "hello")
        }
        #expect(resolution == .keepLocal)
        #expect(!session.isComposing(for: nodeID))
        #expect(session.localValue(for: nodeID) == "hello!")
        #expect(commits.isEmpty)
    }

    @Test("First local edit on an unpublished editor seeds an empty baseline")
    func firstLocalEditSeedsEmptyAuthoritativeBaseline() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.lastKnownAuthoritative(for: nodeID) == nil)
        session.noteLocalValue("too long", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(session.lastKnownAuthoritative(for: nodeID) == "")
        #expect(commits == ["too long"])

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "") == .apply)
        #expect(session.localValue(for: nodeID) == "")
    }

    @Test("A remount of an unpublished editor keeps a local draft against empty")
    func remountOfUnpublishedEditorKeepsDraft() {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        session.noteLocalValue("draft", nodeID: nodeID, composing: false, flushImmediately: false)
        let resolution = session.withPreservedLocalText {
            session.applyPublishedValue(nodeID: nodeID, published: "")
        }
        #expect(resolution == .keepLocal)
        #expect(session.localValue(for: nodeID) == "draft")
        #expect(commits.isEmpty)
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
        #expect(session.hasUnsentSuccessorDraft(for: nodeID) == false)
    }

    @Test("Authoritative apply resets lastFlushedValue so retyping the previous submit commits")
    func applyResetsLastFlushedValue() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "foo") == .apply)
        session.noteLocalValue("foo", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteAssigned(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "foo",
                editSeq: try #require(EditSeq(1))
            )
        )
        let commitsBeforeCorrection = commits.count

        #expect(session.applyPublishedValue(nodeID: nodeID, published: "bar") == .apply)
        #expect(session.localValue(for: nodeID) == "bar")
        #expect(session.lastKnownAuthoritative(for: nodeID) == "bar")

        session.noteLocalValue("foo", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(commits.count == commitsBeforeCorrection + 1)
        #expect(commits.last == "foo")
    }

    @Test("Echo keepLocal does not reset lastFlushedValue")
    func echoKeepLocalLeavesFlushedBaseline() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        session.noteLocalValue("foo", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteAssigned(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "foo",
                editSeq: try #require(EditSeq(1))
            )
        )
        #expect(commits == ["foo"])
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "foo") == .keepLocal)

        session.noteLocalValue("foo", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(commits == ["foo"])
    }

    @Test("A pending successor is an unsent draft; a lone submit is not")
    func hasUnsentSuccessorDraftDetectsNewerTyping() throws {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "bar") == .apply)
        session.noteLocalValue("foo", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteAssigned(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "foo",
                editSeq: try #require(EditSeq(1))
            )
        )
        #expect(!session.hasUnsentSuccessorDraft(for: nodeID))
        #expect(session.lastKnownAuthoritative(for: nodeID) == "bar")

        session.noteLocalValue("food", nodeID: nodeID, composing: false, flushImmediately: false)
        #expect(session.hasUnsentSuccessorDraft(for: nodeID))
        #expect(session.localValue(for: nodeID) == "food")
    }

    @Test("A flushed successor is an unsent draft until it is assigned")
    func flushedSuccessorCountsAsUnsentDraft() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "bar") == .apply)
        session.noteLocalValue("foo", nodeID: nodeID, composing: false, flushImmediately: true)
        session.noteAssigned(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "foo",
                editSeq: try #require(EditSeq(1))
            )
        )
        session.noteLocalValue("food", nodeID: nodeID, composing: false, flushImmediately: true)
        #expect(session.hasUnsentSuccessorDraft(for: nodeID))
        session.noteAcknowledged(
            Event.textEdit(
                eventSeq: 1,
                eventId: EventId(string: "e1"),
                observedRevision: Revision(1),
                nodeId: nodeID,
                text: "foo",
                editSeq: try #require(EditSeq(1))
            )
        )
        #expect(session.hasUnsentSuccessorDraft(for: nodeID))
        #expect(session.applyPublishedValue(nodeID: nodeID, published: "bar") == .apply)
        #expect(!session.hasUnsentSuccessorDraft(for: nodeID))
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
