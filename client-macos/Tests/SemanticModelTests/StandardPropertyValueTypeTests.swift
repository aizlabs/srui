import SemanticModel
import Testing

struct StandardPropertyValueTypeTests {
    @Test
    func generatedMetadataMatchesCanonicalGrowAndShrinkTypes() {
        #expect(standardPropertyValueType(.grow) == .float64)
        #expect(standardPropertyValueType(.shrink) == .float64)
        #expect(standardPropertyValueType(.value) == .any)
        #expect(
            standardPropertyValueType(PropertyRef(namespaceID: 42, localID: 20)) == nil
        )
    }

    @Test
    func storeRejectsWrongStandardPropertyVariantWithoutMutation() {
        var store = SemanticStore()

        #expect(throws: StoreError.invalidPropertyValueType(
            property: .grow,
            expected: .float64,
            actual: "bool"
        )) {
            try store.createNode(
                id: 1,
                nodeType: .column,
                properties: [(.grow, .bool(true))]
            )
        }

        #expect(store.isEmpty)
    }

    @Test
    func storeAcceptsCanonicalGrowVariant() throws {
        var store = SemanticStore()

        try store.createNode(
            id: 1,
            nodeType: .column,
            properties: [(.grow, .float64(1))]
        )

        #expect(store.getNode(1)?.getProperty(.grow) == .float64(1))
    }
}
