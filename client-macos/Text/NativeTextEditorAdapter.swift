//
// NativeTextEditorAdapter.swift
// Text
//
// Observes AppKit text controls without taking over keyboard, selection, caret,
// clipboard, or text-input-client behavior (§22.6). Immediate editing feedback
// stays in the field editor / text view; SRUI only sees committed whole values.
//

import AppKit
import SemanticModel

/// `@MainActor` observer for `NSTextField` / `NSTextView` editors (§22.6).
@MainActor
public final class NativeTextEditorAdapter: NSObject, NSTextFieldDelegate, NSTextViewDelegate {
    public let nodeID: NodeId
    public private(set) var validationState: StandardValidationState?
    /// Test hook: when non-`nil`, replaces AppKit marked-text detection.
    public var compositionOverride: Bool?

    private weak var session: TextEditingSession?
    private weak var textField: NSTextField?
    private weak var textView: NSTextView?
    private var applyingAuthoritative = false
    private var lastKnownComposing = false
    private var readOnly = false
    private var enabled = true

    public init(nodeID: NodeId, session: TextEditingSession, textField: NSTextField) {
        self.nodeID = nodeID
        self.session = session
        self.textField = textField
        super.init()
        textField.delegate = self
    }

    public init(nodeID: NodeId, session: TextEditingSession, textView: NSTextView) {
        self.nodeID = nodeID
        self.session = session
        self.textView = textView
        super.init()
        textView.delegate = self
    }

    public var currentString: String {
        if let textField {
            return textField.stringValue
        }
        return textView?.string ?? ""
    }

    public var isComposing: Bool {
        if let compositionOverride {
            return compositionOverride
        }
        if let editor = textField?.currentEditor() as? NSTextView {
            return editor.hasMarkedText()
        }
        return textView?.hasMarkedText() ?? false
    }

    public func applyAuthoritative(_ value: Value?) {
        if let value, value.asString == nil {
            return
        }
        applyAuthoritativeString(value?.asString ?? "")
    }

    public func applyAuthoritativeString(_ string: String) {
        guard let session else {
            assignNativeString(string)
            return
        }
        switch session.applyPublishedValue(nodeID: nodeID, published: string) {
        case .apply:
            assignNativeString(string)
        case .keepLocal:
            if let local = session.localValue(for: nodeID), currentString != local {
                assignNativeString(local)
            }
        case .deferred:
            break
        }
    }

    public func applyReadOnly(_ readOnly: Bool) {
        self.readOnly = readOnly
        applyEditability()
    }

    public func applyEnabled(_ enabled: Bool) {
        self.enabled = enabled
        textField?.isEnabled = enabled
        applyEditability()
    }

    private func applyEditability() {
        let editable = enabled && !readOnly
        textField?.isEditable = editable
        textView?.isEditable = editable
        textView?.isSelectable = enabled
    }

    public func applyValidation(_ value: Value?) {
        let state = value?.asEnumToken.flatMap(StandardValidationState.init(enumToken:))
        validationState = state
        let color: NSColor?
        switch state {
        case .warning:
            color = NSColor.systemOrange.withAlphaComponent(0.22)
        case .error:
            color = NSColor.systemRed.withAlphaComponent(0.22)
        default:
            color = nil
        }
        if let textField {
            textField.drawsBackground = true
            textField.backgroundColor = color ?? NSColor.textBackgroundColor
        }
        if let textView {
            textView.drawsBackground = true
            textView.backgroundColor = color ?? NSColor.textBackgroundColor
        }
    }

    public func notifyTextDidChangeForTests() {
        emitLocalChange(flushImmediately: false)
    }

    public func notifyEndEditingForTests() {
        emitEndEditing()
    }

    public func controlTextDidChange(_ obj: Notification) {
        emitLocalChange(flushImmediately: false)
    }

    public func controlTextDidEndEditing(_ obj: Notification) {
        emitEndEditing()
    }

    public func textDidChange(_ notification: Notification) {
        emitLocalChange(flushImmediately: false)
    }

    public func textDidEndEditing(_ notification: Notification) {
        emitEndEditing()
    }

    private func emitLocalChange(flushImmediately: Bool) {
        guard !applyingAuthoritative else { return }
        let composing = isComposing
        let endedComposition = lastKnownComposing && !composing
        lastKnownComposing = composing
        if let applied = session?.setComposing(composing, nodeID: nodeID) {
            assignNativeString(applied)
            return
        }
        session?.noteLocalValue(
            currentString,
            nodeID: nodeID,
            composing: composing,
            flushImmediately: (flushImmediately || endedComposition) && !composing
        )
    }

    private func emitEndEditing() {
        guard !applyingAuthoritative else { return }
        if session?.isPreservingLocalTextAcrossRemount == true {
            lastKnownComposing = false
            return
        }
        lastKnownComposing = false
        if let applied = session?.setComposing(false, nodeID: nodeID) {
            assignNativeString(applied)
            return
        }
        session?.noteLocalValue(
            currentString,
            nodeID: nodeID,
            composing: false,
            flushImmediately: false
        )
        session?.endEditing(nodeID: nodeID)
    }

    private func assignNativeString(_ string: String) {
        if currentString == string {
            return
        }
        applyingAuthoritative = true
        defer { applyingAuthoritative = false }
        if let textField {
            textField.stringValue = string
        }
        if let textView {
            textView.string = string
        }
    }
}
