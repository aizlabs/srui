//
// NativeTextEditorAdapterTests.swift
// RendererAppKitTests
//
// Observer-only AppKit adapters: local feedback, IME suppression, programmatic
// writes, and validation decoration (§22.6).
//

import AppKit
import SemanticModel
import Testing
import Text
@testable import RendererAppKit

@Suite("NativeTextEditorAdapter")
@MainActor
struct NativeTextEditorAdapterTests {
    private func makeField() -> (TextEditingSession, NativeTextEditorAdapter, NSTextField) {
        let session = TextEditingSession(debounceNanoseconds: 0)
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 180, height: 24))
        let adapter = NativeTextEditorAdapter(nodeID: NodeId(12), session: session, textField: field)
        return (session, adapter, field)
    }

    @Test("Typing updates the native field before any commit callback")
    func typingUpdatesNativeFieldImmediately() {
        let (session, adapter, field) = makeField()
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        field.stringValue = "hello"
        adapter.notifyTextDidChangeForTests()
        #expect(adapter.currentString == "hello")
        #expect(field.stringValue == "hello")
        adapter.notifyEndEditingForTests()
        #expect(commits == ["hello"])
    }

    @Test("Marked text, selection, caret, and copy produce no edit event")
    func markedTextSelectionCaretAndCopyProduceNoEdit() {
        let (session, adapter, field) = makeField()
        var commits = 0
        session.onCommit = { _, _, _, _ in commits += 1 }

        adapter.compositionOverride = true
        field.stringValue = "hel"
        adapter.notifyTextDidChangeForTests()
        #expect(commits == 0)
        #expect(field.stringValue == "hel")

        if let editor = field.currentEditor() {
            editor.selectedRange = NSRange(location: 1, length: 1)
            #expect(editor.selectedRange.location == 1)
        }
        #expect(commits == 0)

        adapter.compositionOverride = false
        adapter.notifyEndEditingForTests()
        #expect(commits == 1)
    }

    @Test("Committed IME candidate is flushed instead of the previous marked string")
    func compositionEndFlushesCommittedCandidate() {
        let (session, adapter, field) = makeField()
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        adapter.compositionOverride = true
        field.stringValue = "hel"
        adapter.notifyTextDidChangeForTests()
        #expect(commits.isEmpty)

        adapter.compositionOverride = false
        field.stringValue = "hello"
        adapter.notifyTextDidChangeForTests()
        #expect(commits == ["hello"])
    }

    @Test("Programmatic authoritative writes do not emit another edit")
    func programmaticWriteDoesNotEmitEdit() {
        let (session, adapter, field) = makeField()
        var commits = 0
        session.onCommit = { _, _, _, _ in commits += 1 }

        adapter.applyAuthoritativeString("from-server")
        #expect(field.stringValue == "from-server")
        #expect(commits == 0)

        adapter.applyAuthoritativeString("from-server")
        #expect(field.stringValue == "from-server")
        #expect(commits == 0)
    }

    @Test("Paste is a committed whole-value change")
    func pasteEmitsWholeValue() {
        let (session, adapter, field) = makeField()
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        field.stringValue = "pasted"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        #expect(commits == ["pasted"])
    }

    @Test("Validation decoration retains semantic state")
    func validationDecorationRetainsSemanticState() {
        let (_, adapter, field) = makeField()
        adapter.applyValidation(.enumToken(StandardValidationState.warning.enumToken))
        #expect(adapter.validationState == .warning)
        adapter.applyValidation(.enumToken(StandardValidationState.error.enumToken))
        #expect(adapter.validationState == .error)
        adapter.applyValidation(.enumToken(StandardValidationState.valid.enumToken))
        #expect(adapter.validationState == .valid)
        adapter.applyValidation(nil)
        #expect(adapter.validationState == nil)
        #expect(field.backgroundColor == NSColor.textBackgroundColor)
    }

    @Test("Text view adapter observes textDidChange")
    func textViewAdapterObservesChanges() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        let view = NSTextView(frame: NSRect(x: 0, y: 0, width: 280, height: 88))
        let adapter = NativeTextEditorAdapter(nodeID: NodeId(14), session: session, textView: view)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        view.string = "area"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        #expect(commits == ["area"])
        #expect(adapter.currentString == "area")
    }

    @Test("End-editing during remount preservation does not flush")
    func endEditingDuringRemountPreserveDoesNotFlush() {
        let session = TextEditingSession(debounceNanoseconds: 1_000_000_000)
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 180, height: 24))
        let adapter = NativeTextEditorAdapter(nodeID: NodeId(12), session: session, textField: field)
        var commits: [String] = []
        session.onCommit = { _, text, _, _ in commits.append(text) }

        #expect(session.applyPublishedValue(nodeID: NodeId(12), published: "hello") == .apply)
        field.stringValue = "hello!"
        adapter.notifyTextDidChangeForTests()

        session.withPreservedLocalText {
            adapter.notifyEndEditingForTests()
        }
        #expect(commits.isEmpty)
        #expect(session.localValue(for: NodeId(12)) == "hello!")
    }
}
