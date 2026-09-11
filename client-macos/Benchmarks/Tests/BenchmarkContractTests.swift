import Testing
@testable import BenchmarkDriver

@Test("percentile uses explicit nearest-index half-up ties")
func percentileHalfUpTie() {
    #expect(percentile([0.0, 1.0], 0.5) == 1.0)
    #expect(percentile([0.0, 1.0, 2.0], 0.25) == 1.0)
}

@Test("percentile rejects empty, nonfinite, and invalid inputs")
func percentileRejectsInvalidInputs() {
    #expect(checkedPercentile([], 0.5) == .failure(.noSamples))
    #expect(
        checkedPercentile([1.0, .nan], 0.5)
            == .failure(.nonfiniteSamples)
    )
    #expect(
        checkedPercentile([1.0], -.infinity)
            == .failure(.invalidFraction)
    )
    #expect(
        checkedPercentile([1.0], 1.1)
            == .failure(.invalidFraction)
    )
}

@Test("fixture roles resolve by semantic type instead of numeric convention")
func fixtureRolesResolveByType() throws {
    let fixture = Fixture(
        name: "typed roles",
        firstPaintNodeCount: 1,
        roles: FixtureRoles(
            surface: 101,
            progress: 303,
            fileTree: 202,
            textEditor: 505,
            primaryAction: 404
        ),
        nodes: [
            FixtureNode(id: 101, type: "Surface", parent: nil, properties: nil),
            FixtureNode(id: 202, type: "Tree", parent: 101, properties: nil),
            FixtureNode(id: 303, type: "Progress", parent: 101, properties: nil),
            FixtureNode(id: 404, type: "Button", parent: 101, properties: nil),
            FixtureNode(id: 505, type: "TextArea", parent: 101, properties: nil),
        ]
    )

    let index = try BenchmarkFixtureIndex(fixture: fixture)
    #expect(index.surface.value == 101)
    #expect(index.progress.value == 303)
    #expect(index.fileTree.value == 202)
    #expect(index.textEditor.value == 505)
    #expect(index.primaryAction.value == 404)
}

@Test("fixture roles reject a semantic type mismatch")
func fixtureRolesRejectTypeMismatch() {
    let fixture = Fixture(
        name: "invalid roles",
        firstPaintNodeCount: 1,
        roles: FixtureRoles(
            surface: 1,
            progress: 2,
            fileTree: 3,
            textEditor: 4,
            primaryAction: 5
        ),
        nodes: [
            FixtureNode(id: 1, type: "Surface", parent: nil, properties: nil),
            FixtureNode(id: 2, type: "Button", parent: 1, properties: nil),
            FixtureNode(id: 3, type: "Tree", parent: 1, properties: nil),
            FixtureNode(id: 4, type: "TextArea", parent: 1, properties: nil),
            FixtureNode(id: 5, type: "Button", parent: 1, properties: nil),
        ]
    )

    #expect(throws: BenchmarkFailure.self) {
        _ = try BenchmarkFixtureIndex(fixture: fixture)
    }
}
