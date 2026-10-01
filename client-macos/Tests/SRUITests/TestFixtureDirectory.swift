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
    private var paths: Set<String> = []
    private var hookInstalled = false

    func add(_ path: String) {
        lock.lock()
        defer { lock.unlock() }
        paths.insert(path)
        guard !hookInstalled else { return }
        hookInstalled = true
        atexit { FixtureDirectoryRegistry.shared.removeAll() }
    }

    /// Removes one directory and forgets it, so the exit hook can never delete a path that is no
    /// longer this registry's to delete.
    func remove(_ path: String) {
        lock.lock()
        let known = paths.remove(path) != nil
        lock.unlock()
        guard known else { return }
        try? FileManager.default.removeItem(atPath: path)
    }

    func removeAll() {
        lock.lock()
        let pending = paths
        paths.removeAll()
        lock.unlock()
        for path in pending {
            try? FileManager.default.removeItem(atPath: path)
        }
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
enum TestFixtureDirectory {
    /// Creates `/tmp/<prefix>-<unique>` as 0700 and registers it for removal.
    static func make(prefix: String) throws -> URL {
        let directory = reserve(prefix: prefix)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: false
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: directory.path
        )
        return directory
    }

    /// Registers a unique path for removal *without* creating it, for the cases where the
    /// component under test is the one that has to create its own private parent (§27), or where
    /// nothing is ever expected to appear at the path at all.
    static func reserve(prefix: String) -> URL {
        let directory = URL(
            fileURLWithPath: "/tmp/\(prefix)-\(UUID().uuidString.prefix(8))"
        )
        FixtureDirectoryRegistry.shared.add(directory.path)
        return directory
    }

    /// Recursively removes a directory `make` or `reserve` handed out, and deregisters it.
    static func release(_ directory: URL) {
        FixtureDirectoryRegistry.shared.remove(directory.path)
    }
}
