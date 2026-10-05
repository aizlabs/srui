//
// TestFixtureDirectoryTests.swift
// SRUITests
//
// The fixture-directory owner is what keeps generated SSH private keys from being left in /tmp,
// and what keeps its own exit hook from deleting a directory it does not own.
//

import Foundation
import Testing

@Suite("Fixture directory ownership")
struct TestFixtureDirectoryTests {
    @Test("A created fixture directory is private and removed on release")
    func makeCreatesAPrivateDirectoryAndReleaseRemovesIt() throws {
        let first = try TestFixtureDirectory.make(prefix: "test-srui-fixture")
        let second = try TestFixtureDirectory.make(prefix: "test-srui-fixture")
        #expect(first != second, "each fixture needs its own directory")

        for directory in [first, second] {
            let attributes = try FileManager.default.attributesOfItem(atPath: directory.path)
            // 0700 from creation, not set afterwards: keys are written in here.
            #expect((attributes[.posixPermissions] as? NSNumber)?.int16Value == 0o700)
            #expect(attributes[.type] as? FileAttributeType == .typeDirectory)
        }

        // A file inside goes with it: the whole point is that a server's siblings - a
        // `<socket>.lock`, a host key - are cleared along with the endpoint.
        let inside = first.appendingPathComponent("host_key")
        try Data("key material".utf8).write(to: inside)
        TestFixtureDirectory.release(first)
        #expect(!FileManager.default.fileExists(atPath: inside.path))
        #expect(!FileManager.default.fileExists(atPath: first.path))

        TestFixtureDirectory.release(second)
        #expect(!FileManager.default.fileExists(atPath: second.path))
    }

    @Test("A reserved path is not created, and is removed once it is ours")
    func reserveHandsOutAnUnusedPath() throws {
        let reserved = try TestFixtureDirectory.reserve(prefix: "test-srui-reserved")
        #expect(!FileManager.default.fileExists(atPath: reserved.path),
                "reserve must not create the path: the component under test does (§27)")

        // Once the component under test has created it, releasing clears it.
        try FileManager.default.createDirectory(at: reserved, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
        TestFixtureDirectory.release(reserved)
        #expect(!FileManager.default.fileExists(atPath: reserved.path))
    }

    /// A reserved name is a name, not a possession: between the reservation and the
    /// removal, anything could occupy it. Only a directory this user owns may be
    /// recursively removed - a root-owned directory, a plain file or a symlink at that
    /// name belongs to something else, and deleting it is the failure the guard exists
    /// to prevent. Asserted against synthesized attributes so no root-owned path has to
    /// be staged, which an unprivileged test cannot do anyway.
    @Test("Only a directory this user owns may be removed from a reserved name")
    func removalRequiresOwnershipOfADirectory() {
        let ours = NSNumber(value: getuid())
        let root = NSNumber(value: UInt32(0))
        #expect(TestFixtureDirectory.permitsRemoval([
            .ownerAccountID: ours, .type: FileAttributeType.typeDirectory,
        ]))
        #expect(!TestFixtureDirectory.permitsRemoval([
            .ownerAccountID: root, .type: FileAttributeType.typeDirectory,
        ]), "another user's directory must never be removed")
        #expect(!TestFixtureDirectory.permitsRemoval([
            .ownerAccountID: ours, .type: FileAttributeType.typeRegular,
        ]), "a file at a reserved name is not this fixture's directory")
        #expect(!TestFixtureDirectory.permitsRemoval([
            .ownerAccountID: ours, .type: FileAttributeType.typeSymbolicLink,
        ]), "a symlink points somewhere this registry never reserved")
        #expect(!TestFixtureDirectory.permitsRemoval([:]), "no attributes is no evidence")
    }

    /// The predicate above is only worth having if the removal path consults it. This drives the
    /// real `release`, with a plain file at the reserved name - the shape a test cannot stage for
    /// the root-owned case, and the one that proves the guard is wired in rather than merely
    /// present: with the `permitsRemoval` check deleted, `removeItem` unlinks this file.
    /// A reserved name no other live process of this user is reaching for: it carries
    /// this process's pid as well as random bytes. `permitsRemoval` cannot tell our
    /// directory from another of this user's, and for a reserved path nothing can -
    /// the component under test creates it and writes no token of ours - so the name
    /// is where that distinction has to live.
    @Test("A fixture path is scoped to this process, not just randomized")
    func fixturePathsCarryThisProcessIdentity() throws {
        let made = try TestFixtureDirectory.make(prefix: "test-srui-scoped")
        defer { TestFixtureDirectory.release(made) }
        let reserved = try TestFixtureDirectory.reserve(prefix: "test-srui-scoped")
        defer { TestFixtureDirectory.release(reserved) }

        for directory in [made, reserved] {
            #expect(directory.lastPathComponent.hasPrefix("test-srui-scoped-\(getpid())-"),
                    "a fixture path must name the process that reserved it: \(directory.path)")
        }
    }

    @Test("Release refuses a reserved name that something else occupies")
    func releaseWillNotRemoveAForeignOccupant() throws {
        let reserved = try TestFixtureDirectory.reserve(prefix: "test-srui-occupied")
        try Data("not this fixture's".utf8).write(to: reserved)
        defer { try? FileManager.default.removeItem(at: reserved) }

        TestFixtureDirectory.release(reserved)
        #expect(FileManager.default.fileExists(atPath: reserved.path),
                "a file at a reserved name belongs to something else and must survive release")
    }

    /// What the `atexit` hook does, driven directly: a directory whose test never released it is
    /// still cleared - swift-testing can cut a still-unwinding `defer` short, which is the race
    /// this registry exists for - while a reserved name something else occupies is left alone in
    /// the same pass, so the exit hook can never be the thing that deletes another process's path.
    ///
    /// On a registry of this test's own, never `shared`: draining that one mid-run would remove
    /// the fixture directories of every test executing concurrently beside this one. The
    /// *installation* of `atexit` is not provable from inside the run that installs it; the pass
    /// it runs is what is asserted here.
    @Test("The exit pass clears what a test forgot and spares what is not ours")
    func theExitPassDrainsUnderTheSameGuard() throws {
        let registry = FixtureDirectoryRegistry(installsExitHook: false)
        let unique = UUID().uuidString.prefix(8)

        let forgotten = URL(fileURLWithPath: "/tmp/test-srui-forgotten-\(unique)")
        try FileManager.default.createDirectory(at: forgotten, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: forgotten) }
        try Data("key material".utf8).write(to: forgotten.appendingPathComponent("host_key"))
        registry.add(forgotten.path)

        let occupied = URL(fileURLWithPath: "/tmp/test-srui-exit-occupied-\(unique)")
        registry.add(occupied.path, ownedOnly: true)
        try Data("not this fixture's".utf8).write(to: occupied)
        defer { try? FileManager.default.removeItem(at: occupied) }

        registry.removeAll()

        #expect(!FileManager.default.fileExists(atPath: forgotten.path),
                "the exit pass must clear a fixture directory no defer released")
        #expect(FileManager.default.fileExists(atPath: occupied.path),
                "the exit pass must not remove a reserved name something else occupies")
    }
}
