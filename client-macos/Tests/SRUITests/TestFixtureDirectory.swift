//
// TestFixtureDirectory.swift
// SRUITests
//
// One owner for the /tmp fixture directories the socket and SSH integration suites create
// (§19, §19.1, §20.2, §27).
//

import Foundation

/// Fixture directories handed out by `TestFixtureDirectory` and not yet released.
///
/// This mirrors `SSHTestSupport`'s `SSHDRegistry`, for the same reason. The per-test `defer` is
/// the primary removal path and already covers success, an `#expect` failure, a thrown error and
/// an early `return` alike; what it does not cover is swift-testing exiting the test binary as
/// soon as the last test finishes, which can cut a still-unwinding `defer` short. `atexit` runs on
/// that normal exit, so it is the one hook that sees whichever directory lost the race.
///
/// Cleaning up at the source rather than in a sweeper is deliberate: these fixtures hold generated
/// SSH private keys, and `scripts/reap-test-servers.sh` refuses to collect a directory holding
/// anything but sockets precisely so an automated `rm -rf` can never delete key material. That
/// leaves the test that created the directory as the only place the removal may legitimately
/// happen.
private final class FixtureDirectoryRegistry: @unchecked Sendable {
    static let shared = FixtureDirectoryRegistry()

    private let lock = NSLock()
    /// Path to whether removal must first prove the path is ours.
    private var paths: [String: Bool] = [:]
    private var hookInstalled = false

    /// Registers a path for removal. `ownedOnly` paths are removed only if they are
    /// this user's when the time comes: a reserved path is not created here, so
    /// nothing can prove ownership up front, and the registry must not hand an
    /// `rm -rf` a directory that some other process put there.
    func add(_ path: String, ownedOnly: Bool = false) {
        lock.lock()
        defer { lock.unlock() }
        paths[path] = ownedOnly
        guard !hookInstalled else { return }
        hookInstalled = true
        atexit { FixtureDirectoryRegistry.shared.removeAll() }
    }

    /// Removes one directory and forgets it, so the exit hook can never delete a path that is no
    /// longer this registry's to delete.
    func remove(_ path: String) {
        lock.lock()
        let ownedOnly = paths.removeValue(forKey: path)
        lock.unlock()
        guard let ownedOnly else { return }
        Self.removeIfPermitted(path, ownedOnly: ownedOnly)
    }

    func removeAll() {
        lock.lock()
        let pending = paths
        paths.removeAll()
        lock.unlock()
        for (path, ownedOnly) in pending {
            Self.removeIfPermitted(path, ownedOnly: ownedOnly)
        }
    }

    /// Removes `path`, unless it was only reserved and something this user does not
    /// own now sits there.
    ///
    /// A reserved path is a name, not a possession: the component under test creates
    /// it, and between the reservation and the removal any process could have created
    /// it instead. The owner check is what keeps the exit hook from recursively
    /// deleting that directory's contents. A path `make` created needs no check -- it
    /// did not exist a moment earlier, and `createDirectory` would have failed if it
    /// had.
    private static func removeIfPermitted(_ path: String, ownedOnly: Bool) {
        if ownedOnly {
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: path),
                  TestFixtureDirectory.permitsRemoval(attributes)
            else { return }
        }
        try? FileManager.default.removeItem(atPath: path)
    }
}

/// A private `/tmp` fixture directory whose removal is guaranteed on every exit path.
///
/// Every integration suite that needs a socket, an sshd config, a host key or a generated user key
/// puts them *inside* one of these, so a single recursive removal clears the fixture whatever the
/// server chose to write next to its endpoint — `coding-agent-demo`, for instance, writes a
/// `<socket>.lock` sibling that a test removing only its own socket path leaves behind forever.
///
/// Call `release(_:)` from a `defer` declared *immediately* after `make`/`reserve`. Swift runs
/// deferred blocks in reverse declaration order, so declaring this one first makes it run **last**
/// — after the `defer` that stops the fixture server and after `SSHTestSupport.terminate`. The
/// order matters: removing the directory first would unlink a live server's endpoint out from
/// under it while the test is still tearing that server down.
enum FixtureDirectoryError: Error, CustomStringConvertible {
    case noFreeName(prefix: String)

    var description: String {
        switch self {
        case .noFreeName(let prefix):
            return "could not reserve an unused /tmp/\(prefix)-* path after eight attempts"
        }
    }
}

enum TestFixtureDirectory {
    /// Creates `/tmp/<prefix>-<unique>` as 0700 and registers it for removal.
    ///
    /// Ownership is established *before* registration, and that order is the point:
    /// `createDirectory(withIntermediateDirectories: false)` fails if anything is
    /// already at the path, so a registered path is one this process created. Doing it
    /// the other way round registered the path first, and a pre-existing directory --
    /// a developer's, or another run's -- was then recursively removed by the exit hook
    /// even though `make` had thrown. The mode is set in the creation attributes rather
    /// than afterwards, so the directory is never briefly world-readable while keys are
    /// about to be written into it.
    static func make(prefix: String) throws -> URL {
        let directory = URL(fileURLWithPath: "/tmp/\(prefix)-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700]
        )
        FixtureDirectoryRegistry.shared.add(directory.path)
        return directory
    }

    /// Registers a unique path for removal *without* creating it, for the cases where the
    /// component under test is the one that has to create its own private parent (§27), or where
    /// nothing is ever expected to appear at the path at all.
    ///
    /// Nothing here can prove ownership -- that is the whole point of not creating it --
    /// so the path is registered as removable only while it belongs to this user, and
    /// the reservation refuses a name that is already taken.
    static func reserve(prefix: String) throws -> URL {
        for _ in 0..<8 {
            let directory = URL(fileURLWithPath: "/tmp/\(prefix)-\(UUID().uuidString.prefix(8))")
            if !FileManager.default.fileExists(atPath: directory.path) {
                FixtureDirectoryRegistry.shared.add(directory.path, ownedOnly: true)
                return directory
            }
        }
        throw FixtureDirectoryError.noFreeName(prefix: prefix)
    }

    /// Recursively removes a directory `make` or `reserve` handed out, and deregisters it.
    static func release(_ directory: URL) {
        FixtureDirectoryRegistry.shared.remove(directory.path)
    }

    /// Whether a *reserved* path, now occupied, may be recursively removed.
    ///
    /// Only a directory this user owns. Anything else at a reserved name belongs to
    /// something other than the test that reserved it -- another user's, or a file or
    /// symlink pointing somewhere entirely different -- and an `rm -rf` of it is the
    /// failure this predicate exists to prevent. Exposed so the decision can be tested
    /// without staging a root-owned path.
    static func permitsRemoval(_ attributes: [FileAttributeKey: Any]) -> Bool {
        (attributes[.ownerAccountID] as? NSNumber)?.uint32Value == getuid()
            && attributes[.type] as? FileAttributeType == .typeDirectory
    }
}
