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
}
