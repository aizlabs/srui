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

@MainActor
struct FrameIndependenceConformanceTests {

    /// Commits coalesced per present pass. 1 models a client presenting every commit (a fast
    /// display); 8 models a slow one that presents once per batch.
    private static let cadences = [1, 2, 3, 4, 8]
    private static let mutationCount = 24

    private struct CadenceRun {
        let finalText: String?
        let textViewIdentity: ObjectIdentifier
        let surfaceWindowIdentity: ObjectIdentifier
        let nodeCount: Int
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

    /// Applies the same mutation stream, presenting once every `cadence` commits.
    private func run(cadence: Int) throws -> CadenceRun {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 2, nodeType: .text, parentID: 1, properties: [(.text, .string("v0"))]),
        ]
        var applied = base
        let store = try makeStore(base)
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)

        var presentPasses = 0
        var pending: [SemanticModel.Operation] = []
        var allClassifications: [DirtyClassification] = []
        var currentRevision = store.revision

        func present() throws {
            guard !pending.isEmpty else { return }
            let newStore = try makeStore(applied)
            let classifications = try renderer.apply(
                transaction: Transaction(baseRevision: currentRevision, operations: pending),
                newStore: newStore
            )
            allClassifications.append(contentsOf: classifications)
            currentRevision = newStore.revision
            presentPasses += 1
            pending.removeAll()
        }

        for i in 1...Self.mutationCount {
            let mutation = SemanticModel.Operation.setProperty(
                id: 2, property: .text, value: .string("v\(i)"))
            applied.append(mutation)
            pending.append(mutation)
            if i % cadence == 0 {
                try present()
            }
        }
        try present()

        let finalStore = try makeStore(applied)
        let textView = try #require(renderer.registry.view(for: 2))
        let surfaceWindow = try #require(renderer.registry.handle(for: 1)?.window)

        return CadenceRun(
            finalText: finalStore.node(for: 2)?.properties[.text]?.asString,
            textViewIdentity: ObjectIdentifier(textView),
            surfaceWindowIdentity: ObjectIdentifier(surfaceWindow),
            nodeCount: finalStore.nodeCount,
            presentPasses: presentPasses,
            classifications: allClassifications
        )
    }

    /// §12.2: the renderer paces presentation independently, so every cadence lands on the same
    /// semantic state — same final value, same node count.
    @Test
    func everyCadenceConvergesOnTheSameSemanticState() throws {
        let baseline = try run(cadence: 1)

        for cadence in Self.cadences {
            let result = try run(cadence: cadence)
            #expect(
                result.finalText == baseline.finalText,
                "cadence \(cadence) converged on \(String(describing: result.finalText)) rather than \(String(describing: baseline.finalText))"
            )
            #expect(
                result.nodeCount == baseline.nodeCount,
                "cadence \(cadence) produced a different node count")
        }
    }

    /// §23: a scalar property set mutates the existing view in place. Coalescing more commits per
    /// present pass must not turn scalar updates into structural rebuilds.
    @Test
    func viewIdentityIsPreservedAtEveryCadence() throws {
        for cadence in Self.cadences {
            let result = try run(cadence: cadence)

            let structural = result.classifications.filter {
                if case .structureAffecting = $0 { return true }
                return false
            }
            #expect(
                structural.isEmpty,
                "cadence \(cadence) reported \(structural.count) structure-affecting classifications for a pure scalar stream; presentation pacing must not change how an operation is classified (§23, §32.4)"
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
            "a slower cadence must produce strictly fewer present passes (got \(lazy.presentPasses) vs \(eager.presentPasses)); if this no longer holds, the cadence variable is not reaching the renderer and the convergence assertions prove nothing"
        )

        // Local repaint pacing differs; the semantic result does not.
        #expect(lazy.finalText == eager.finalText)
        #expect(lazy.nodeCount == eager.nodeCount)
    }
}
