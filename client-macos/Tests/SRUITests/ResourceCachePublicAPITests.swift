//
// ResourceCachePublicAPITests.swift
// SRUITests
//
// Compile-time coverage of the unmanaged API exposed by the Resources product.
//

import Resources
import Testing

@Suite("ResourceCache Public API Tests")
struct ResourceCachePublicAPITests {
    @Test("External callers retain the unmanaged cache lifecycle")
    func unmanagedLifecycleRemainsPublic() async {
        let cache = ResourceCache()

        await cache.setLiveReferences([])
        await cache.clearPartials()

        #expect(await cache.knownHashes().isEmpty)
        #expect(await cache.committedCount() == 0)
    }
}
