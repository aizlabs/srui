//
// SecurityLimitsConformanceTests.swift
// RendererAppKitTests
//
// SRUI Security Limits Conformance Suite (§32 item 9) — renderer half.
//
// Implements: §26 (limits), §27 (the client executes no server-supplied code),
// §4 inv. 10/18, §32.9.
//
// The Rust half proves the store refuses oversized/deep payloads and that script-shaped strings
// stay inert data. This half proves the same payloads stay inert *after* they reach AppKit: the
// renderer is where a script-looking string would have to find an evaluator, a web view, or a
// subprocess in order to do harm.
//

import AppKit
import Foundation
import SemanticModel
import Testing

@testable import RendererAppKit

// A renderer test that blocks on the window server would otherwise pin at 0% CPU
// forever; bound it so a hang is a failure, not a stalled run.
@Suite(.timeLimit(.minutes(1)))
@MainActor
struct SecurityLimitsConformanceTests {

    private static let scriptShapedPayloads = [
        "<script>alert('x')</script>",
        "javascript:void(0)",
        "${jndi:ldap://example.invalid/a}",
        "'; DROP TABLE nodes; --",
        "$(rm -rf /)",
        "data:text/html;base64,PHNjcmlwdD4=",
        "\u{1b}]0;title\u{7}",
    ]

    /// §27: a script-shaped label reaches the native control as literal text. Nothing parses it,
    /// and it is not interpreted as markup, a URL, or an attributed-string directive.
    @Test
    func scriptShapedTextRendersAsLiteralStringContent() throws {
        let factory = ControlFactory()

        for payload in Self.scriptShapedPayloads {
            let handle = try factory.makeHandle(
                for: Node(id: 1, nodeType: .text, properties: [(.text, .string(payload))]))
            let field = try #require(handle.view as? NSTextField)

            #expect(
                field.stringValue == payload,
                "a script-shaped payload must reach the control as literal text (§27)")
            #expect(
                field.isEditable == false,
                "a Text node is not an input surface")
            // Rendered as plain text, never as parsed markup.
            #expect(field.attributedStringValue.string == payload)
        }
    }

    /// §27 / §4 inv. 10: the renderer builds no execution surface. Every view the factory can
    /// produce for a required-tier node is an ordinary AppKit control — never a web view, script
    /// host, or anything with an `evaluate`/`load`-style dispatch entry point.
    @Test
    func rendererConstructsNoExecutionSurface() throws {
        let factory = ControlFactory()
        let requiredTier: [TypeRef] = [
            .surface, .row, .column, .grid, .spacer, .separator, .scroll,
            .text, .richText, .button, .toggle, .textInput, .textArea,
            .progress, .image, .list, .table, .tree,
        ]

        let forbiddenClassFragments = [
            "WebView", "WKWebView", "JSContext", "Script", "Interpreter", "Task", "Process",
        ]

        for nodeType in requiredTier {
            let handle = try factory.makeHandle(for: Node(id: 1, nodeType: nodeType))
            var pending: [NSView] = [handle.view]
            while let view = pending.popLast() {
                let className = String(describing: type(of: view))
                for fragment in forbiddenClassFragments {
                    #expect(
                        !className.contains(fragment),
                        "\(nodeType) produced '\(className)', which looks like an execution surface (§27, §4 inv. 10)"
                    )
                }
                pending.append(contentsOf: view.subviews)
            }
        }
    }

    /// §27: a script-shaped string in an *editable* control is still just text. It round-trips
    /// through the native editor unchanged and is never evaluated on the way in or out.
    @Test
    func scriptShapedTextRoundTripsThroughEditableControls() throws {
        let factory = ControlFactory()

        for payload in Self.scriptShapedPayloads {
            let handle = try factory.makeHandle(
                for: Node(id: 2, nodeType: .textInput, properties: [(.text, .string(payload))]))
            let field = try #require(handle.view as? NSTextField)

            #expect(field.stringValue == payload)
            #expect(
                field.cell?.formatter == nil,
                "no formatter may reinterpret server-supplied text (§27)")
        }
    }

    /// §26: an oversized string is a decode-time concern, but the renderer must also not choke
    /// on one that is merely large — it is displayed as data, not parsed.
    @Test
    func largeTextPayloadRemainsInertData() throws {
        let factory = ControlFactory()
        let large = String(repeating: "A", count: 100_000)

        let handle = try factory.makeHandle(
            for: Node(id: 3, nodeType: .text, properties: [(.text, .string(large))]))
        let field = try #require(handle.view as? NSTextField)

        #expect(field.stringValue.count == large.count)
    }
}
