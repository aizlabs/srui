//
// ConformanceFixtures.swift
// RendererAppKitTests
//
// Locates the generated §32 conformance fixtures for the renderer-side suites (items 2 and 12).
//
// A deliberately small counterpart to SemanticModelTests/ConformanceManifest.swift: Swift test
// targets cannot share code, and the renderer suites consume generated fixtures rather than
// count-pinned vector directories, so only path resolution and decoding are needed here.
//

import Foundation

struct WidgetMatrix: Decodable {
    let nodeTypes: [Row]

    enum CodingKeys: String, CodingKey {
        case nodeTypes = "node_types"
    }

    struct Row: Decodable {
        let id: Int
        let name: String
        let tier: String
        let category: String
        let emits: [String]
        let expectConstructible: Bool

        enum CodingKeys: String, CodingKey {
            case id, name, tier, category, emits
            case expectConstructible = "expect_constructible"
        }
    }
}

struct ToolkitMappings: Decodable {
    let normative: Bool
    let mappings: [Row]

    struct Row: Decodable {
        let node: String
        let tier: String
        let category: String
        let appkitView: String
        let appkitStrategy: String
        let implemented: Bool

        enum CodingKeys: String, CodingKey {
            case node, tier, category, implemented
            case appkitView = "appkit_view"
            case appkitStrategy = "appkit_strategy"
        }
    }
}

enum ConformanceFixtureError: Error, CustomStringConvertible {
    case vectorsRootNotFound

    var description: String {
        switch self {
        case .vectorsRootNotFound:
            return "Could not locate protocol/conformance-vectors"
        }
    }
}

enum ConformanceFixtures {
    static func root() throws -> URL {
        let fromSource = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // Tests/RendererAppKitTests
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
        throw ConformanceFixtureError.vectorsRootNotFound
    }

    static func widgetMatrix() throws -> WidgetMatrix {
        let url = try root().appendingPathComponent(
            "suites/02-widget-semantics/widgets.generated.json")
        return try JSONDecoder().decode(WidgetMatrix.self, from: Data(contentsOf: url))
    }

    static func toolkitMappings() throws -> ToolkitMappings {
        let url = try root().appendingPathComponent(
            "suites/12-toolkit-mapping/mappings.generated.json")
        return try JSONDecoder().decode(ToolkitMappings.self, from: Data(contentsOf: url))
    }
}
