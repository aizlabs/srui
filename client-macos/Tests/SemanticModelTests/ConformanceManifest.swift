//
// ConformanceManifest.swift
// SemanticModelTests
//
// Shared loader for the §32 conformance suite manifest
// (protocol/conformance-vectors/suites/manifest.json).
//
// Vector discovery goes through `vectors(forSuite:)` rather than a bare directory scan so a
// suite's fixture count is pinned exactly. A non-empty check would let a reorganization silently
// drop most of a suite and still report green.
//

import Foundation
import XCTest

struct ConformanceManifest: Decodable {
    let version: Int
    let suites: [Suite]

    struct Suite: Decodable {
        let id: Int
        let slug: String
        let name: String
        let specSections: [String]
        let status: String
        let vectors: Vectors?
        let gaps: [Gap]?

        enum CodingKeys: String, CodingKey {
            case id, slug, name, status, vectors, gaps
            case specSections = "spec_sections"
        }
    }

    struct Vectors: Decodable {
        let dir: String
        /// Exact number of JSON vectors expected in `dir`. Absent for code-driven suites.
        let count: Int?
        /// Generated fixture file names expected in `dir`.
        let generated: [String]?
    }

    struct Gap: Decodable {
        let scenario: String
        let reason: String
        let futureTask: String

        enum CodingKeys: String, CodingKey {
            case scenario, reason
            case futureTask = "future_task"
        }
    }
}

enum ConformanceVectors {
    /// `protocol/conformance-vectors/`, resolved from this source file's location.
    ///
    /// Falls back to the working directory so the suite runs both from SwiftPM and from a
    /// repository-root invocation.
    static func root() -> URL? {
        let fromSource = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // Tests/SemanticModelTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // client-macos
            .deletingLastPathComponent()  // repo root
            .appendingPathComponent("protocol/conformance-vectors")
        if FileManager.default.fileExists(atPath: fromSource.path) {
            return fromSource
        }

        let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        for candidate in ["protocol/conformance-vectors", "../protocol/conformance-vectors"] {
            let url = cwd.appendingPathComponent(candidate)
            if FileManager.default.fileExists(atPath: url.path) {
                return url
            }
        }
        return nil
    }

    static func loadManifest(file: StaticString = #filePath, line: UInt = #line) throws
        -> ConformanceManifest
    {
        let root = try XCTUnwrap(
            root(), "Could not locate protocol/conformance-vectors", file: file, line: line)
        let data = try Data(contentsOf: root.appendingPathComponent("suites/manifest.json"))
        return try JSONDecoder().decode(ConformanceManifest.self, from: data)
    }

    static func suite(_ id: Int, file: StaticString = #filePath, line: UInt = #line) throws
        -> ConformanceManifest.Suite
    {
        let manifest = try loadManifest(file: file, line: line)
        return try XCTUnwrap(
            manifest.suites.first { $0.id == id },
            "Conformance manifest has no suite with id \(id)", file: file, line: line)
    }

    /// Sorted `.json` vectors for `suiteID`, asserted to match the manifest count exactly.
    ///
    /// Both directions matter: a missing file means coverage was lost in a move, an extra file
    /// means a fixture exists that no runner accounts for.
    static func vectors(forSuite suiteID: Int, file: StaticString = #filePath, line: UInt = #line)
        throws -> [URL]
    {
        let root = try XCTUnwrap(
            root(), "Could not locate protocol/conformance-vectors", file: file, line: line)
        let suite = try suite(suiteID, file: file, line: line)
        let vectors = try XCTUnwrap(
            suite.vectors, "Suite \(suiteID) declares no vectors in the manifest", file: file,
            line: line)
        let expected = try XCTUnwrap(
            vectors.count, "Suite \(suiteID) declares no exact vector count in the manifest",
            file: file, line: line)

        let dir = root.appendingPathComponent(vectors.dir)
        let found = try FileManager.default
            .contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }

        XCTAssertEqual(
            found.count, expected,
            """
            Suite \(suiteID) (\(suite.slug)) declares exactly \(expected) vectors in the manifest \
            but \(dir.path) contains \(found.count). Update \
            protocol/conformance-vectors/suites/manifest.json in the same commit that adds or \
            removes a fixture.
            """,
            file: file, line: line)

        return found
    }

    /// Absolute URL of a generated fixture declared by `suiteID`.
    static func generated(
        forSuite suiteID: Int, named fileName: String, file: StaticString = #filePath,
        line: UInt = #line
    ) throws -> URL {
        let root = try XCTUnwrap(
            root(), "Could not locate protocol/conformance-vectors", file: file, line: line)
        let suite = try suite(suiteID, file: file, line: line)
        let vectors = try XCTUnwrap(
            suite.vectors, "Suite \(suiteID) declares no vectors in the manifest", file: file,
            line: line)
        XCTAssertTrue(
            vectors.generated?.contains(fileName) ?? false,
            "Suite \(suiteID) does not declare generated fixture '\(fileName)' in the manifest",
            file: file, line: line)
        return root.appendingPathComponent(vectors.dir).appendingPathComponent(fileName)
    }
}
