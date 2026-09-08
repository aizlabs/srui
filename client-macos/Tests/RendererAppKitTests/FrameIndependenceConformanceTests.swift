//
// FrameIndependenceConformanceTests.swift
// RendererAppKitTests
//
// SRUI Frame-Independence Conformance Suite (§32 item 4) — renderer side.
//
// Implements: §4.16 (no frame cadence in the protocol), §12.2 (commits are state-consistency
// boundaries, not render frames), §23 (macOS renderer performance strategy), §32.4.
//
// The server half of this suite lives in
// server-rust/sessiond/tests/conformance_frame_independence_test.rs and proves that committed
// wire bytes do not vary with how fast a client reads. This half supplies the piece that needs a
// real renderer: presentation is paced locally and independently of commits.
//
// A synthetic "refresh cadence" here is how many committed transactions the renderer coalesces
// before it presents. The assertions are deliberately two-sided:
//
//   * the semantic outcome — final store state and view identity — is identical at every cadence;
//   * the number of present passes genuinely differs across cadences.
//
// Without the second assertion the first would be vacuous: a harness that never actually varied
// anything would report "identical" no matter what the renderer did.
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
struct FrameIndependenceConformanceTests {

    /// Commits coalesced per present pass. 1 models a client presenting every commit (a fast
    /// display); 8 models a slow one that presents once per batch.
    private static let cadences = [1, 2, 3, 4, 8]
    private static let mutationCount = 24

    private struct CadenceRun {
        /// Read from the live NSTextField, not from a rebuilt store: a renderer that drops
        /// updates would still satisfy an assertion made against a reconstructed store.
        let renderedText: String?
        let identitiesStable: Bool
        let viewCount: Int
        let presentPasses: Int
        let classifications: [DirtyClassification]
    }

    private func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
        var store = SemanticStore()
        for operation in operations {
            try operation.apply(to: &store)
        }
        return store
    }

    /// Applies one fixed, cadence-independent transaction stream, varying only how often the
    /// renderer is asked to *present* what it has already applied.
    ///
    /// Transaction boundaries are deliberately held constant: batching mutations into
    /// cadence-sized transactions would change the semantic commit stream itself, so behaviour
    /// wrongly coupled to transaction or revision boundaries could pass a "frame-independence"
    /// suite that never varied presentation at all.
    private func run(cadence: Int) throws -> CadenceRun {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 2, nodeType: .text, parentID: 1, properties: [(.text, .string("v0"))]),
        ]
        let store = try makeStore(base)
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)

        // Captured immediately after mount, so every presentation can be checked against them.
        let initialTextView = try #require(renderer.registry.view(for: 2))
        let initialWindow = try #require(renderer.registry.handle(for: 1)?.window)
        let initialTextIdentity = ObjectIdentifier(initialTextView)
        let initialWindowIdentity = ObjectIdentifier(initialWindow)

        var applied = base
        var presentPasses = 0
        var identitiesStable = true
        var allClassifications: [DirtyClassification] = []
        var currentRevision = store.revision

        // Presentation: force the layout pass over what has already been applied. This is the
        // only thing `cadence` controls.
        //
        // Deliberately *not* `displayIfNeeded()`: real drawing blocks on the window server in an
        // unattended test process, and drawing is not what this suite is about. Layout is the
        // observable per-present work; whether AppKit then rasterises is its own business.
        func present() {
            initialWindow.contentView?.layoutSubtreeIfNeeded()
            presentPasses += 1
        }

        for i in 1...Self.mutationCount {
            // One mutation, one transaction — identical at every cadence.
            let mutation = SemanticModel.Operation.setProperty(
                id: 2, property: .text, value: .string("v\(i)"))
            applied.append(mutation)
            let newStore = try makeStore(applied)

            allClassifications.append(
                contentsOf: try renderer.apply(
                    transaction: Transaction(baseRevision: currentRevision, operations: [mutation]),
                    newStore: newStore))
            currentRevision = newStore.revision

            // §23: a scalar set mutates the view in place. Checked after every apply, so a
            // rebuild-then-restore cycle cannot slip through.
            if let view = renderer.registry.view(for: 2),
               let window = renderer.registry.handle(for: 1)?.window {
                if ObjectIdentifier(view) != initialTextIdentity
                    || ObjectIdentifier(window) != initialWindowIdentity {
                    identitiesStable = false
                }
            } else {
                identitiesStable = false
            }

            if i % cadence == 0 { present() }
        }
        if Self.mutationCount % cadence != 0 { present() }

        // Interrogate the native control the user would actually see.
        let textField = try #require(renderer.registry.view(for: 2) as? NSTextField)

        return CadenceRun(
            renderedText: textField.stringValue,
            identitiesStable: identitiesStable,
            viewCount: renderer.registry.allHandles.count,
            presentPasses: presentPasses,
            classifications: allClassifications
        )
    }

    /// §12.2: the renderer paces presentation independently, so every cadence must leave the
    /// *native control* showing the same thing — read from the NSTextField, not from a store the
    /// test rebuilt for itself.
    @Test
    func everyCadenceRendersTheSameFinalState() throws {
        let baseline = try run(cadence: 1)
        #expect(
            baseline.renderedText == "v\(Self.mutationCount)",
            "the eager cadence must render the last mutation, got \(String(describing: baseline.renderedText))"
        )

        for cadence in Self.cadences {
            let result = try run(cadence: cadence)
            #expect(
                result.renderedText == baseline.renderedText,
                "cadence \(cadence) rendered \(String(describing: result.renderedText)) rather than \(String(describing: baseline.renderedText)); a renderer that drops updates under coalescing would show up here"
            )
            #expect(
                result.viewCount == baseline.viewCount,
                "cadence \(cadence) produced a different number of live view handles")
            #expect(
                result.classifications == baseline.classifications,
                "cadence \(cadence) classified the identical transaction stream differently; presentation pacing must not reach the semantic apply path (§12.2, §32.4)"
            )
        }
    }

    /// §23: a scalar property set mutates the existing view in place. Identities are compared
    /// after every presentation, so coalescing more commits per pass may not turn a scalar update
    /// into a view rebuild.
    @Test
    func viewIdentityIsPreservedAtEveryCadence() throws {
        for cadence in Self.cadences {
            let result = try run(cadence: cadence)

            #expect(
                result.identitiesStable,
                "cadence \(cadence) replaced the text view or surface window during presentation; a scalar stream must mutate in place (§23, §32.4)"
            )

            let structural = result.classifications.filter {
                if case .structureAffecting = $0 { return true }
                return false
            }
            #expect(
                structural.isEmpty,
                "cadence \(cadence) reported \(structural.count) structure-affecting classifications for a pure scalar stream"
            )
        }
    }

    /// The assertion that keeps the two above from being vacuous: the cadence variable really is
    /// wired into the run, and it changes local repaint work — and *only* local repaint work.
    @Test
    func presentPassCountVariesWithCadenceWhileSemanticsDoNot() throws {
        let eager = try run(cadence: 1)
        let lazy = try run(cadence: 8)

        #expect(
            eager.presentPasses == Self.mutationCount,
            "presenting every commit must produce one pass per mutation")
        #expect(
            lazy.presentPasses < eager.presentPasses,
            "a slower cadence must produce strictly fewer present passes (got \(lazy.presentPasses) vs \(eager.presentPasses)); if this no longer holds, the cadence variable is not reaching the renderer and the assertions above prove nothing"
        )

        // Local repaint pacing differs; what the user ends up seeing does not.
        #expect(lazy.renderedText == eager.renderedText)
        #expect(lazy.viewCount == eager.viewCount)
        #expect(lazy.identitiesStable && eager.identitiesStable)
    }
}
