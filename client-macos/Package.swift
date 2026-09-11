// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SRUIClient",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .library(name: "TransportSSH", targets: ["TransportSSH"]),
        .library(name: "Protocol", targets: ["Protocol"]),
        .library(name: "SemanticModel", targets: ["SemanticModel"]),
        .library(name: "Session", targets: ["Session"]),
        .library(name: "RendererAppKit", targets: ["RendererAppKit"]),
        .library(name: "Collections", targets: ["Collections"]),
        .library(name: "Text", targets: ["Text"]),
        .library(name: "Terminal", targets: ["Terminal"]),
        .library(name: "Resources", targets: ["Resources"]),
        .library(name: "Accessibility", targets: ["Accessibility"]),
        .executable(name: "RendererDemoApp", targets: ["RendererDemoApp"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.28.2"),
        .package(path: "LogicalChannelScheduling"),
    ],
    targets: [
        .target(
            name: "TransportSSH",
            dependencies: [
                .product(name: "LogicalChannelScheduling", package: "LogicalChannelScheduling"),
            ],
            path: "TransportSSH"
        ),
        .target(
            name: "SemanticModel",
            path: "SemanticModel"
        ),
        .target(
            name: "Protocol",
            dependencies: [
                "SemanticModel",
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ],
            path: "Protocol"
        ),
        .target(
            name: "Resources",
            dependencies: [
                "SemanticModel",
            ],
            path: "Resources"
        ),
        .target(
            name: "RendererAppKit",
            dependencies: [
                "SemanticModel",
                "Resources",
                "Collections",
                "Text",
                "Terminal",
            ],
            path: "RendererAppKit"
        ),
        .target(
            name: "Session",
            dependencies: [
                "TransportSSH",
                "Protocol",
                "SemanticModel",
                "Accessibility",
                "RendererAppKit",
                "Resources",
                "Collections",
                "Text",
                "Terminal",
            ],
            path: "Session"
        ),
        .executableTarget(
            name: "RendererDemoApp",
            dependencies: [
                "Session",
                "RendererAppKit",
                "TransportSSH",
                "Protocol",
                "SemanticModel",
                "Resources",
            ],
            path: "RendererDemoApp"
        ),
        .target(
            name: "Collections",
            dependencies: [
                "SemanticModel",
            ],
            path: "Collections"
        ),
        .target(
            name: "Text",
            dependencies: [
                "SemanticModel",
            ],
            path: "Text"
        ),
        .target(
            name: "Terminal",
            dependencies: [
                "SemanticModel",
            ],
            path: "Terminal"
        ),
        .target(
            name: "Accessibility",
            dependencies: [
                "SemanticModel",
            ],
            path: "Accessibility"
        ),
        .testTarget(
            name: "SemanticModelTests",
            dependencies: [
                "SemanticModel",
                "Protocol",
            ],
            path: "Tests/SemanticModelTests"
        ),
        .testTarget(
            name: "RendererAppKitTests",
            dependencies: [
                "RendererAppKit",
                "SemanticModel",
                "Resources",
                "Collections",
                "Text",
                "Terminal",
            ],
            path: "Tests/RendererAppKitTests"
        ),
        .testTarget(
            name: "TerminalTests",
            dependencies: [
                "Terminal",
                "SemanticModel",
            ],
            path: "Tests/TerminalTests"
        ),
        .testTarget(
            name: "TextTests",
            dependencies: [
                "Text",
                "SemanticModel",
            ],
            path: "Tests/TextTests"
        ),
        .testTarget(
            name: "ResourcesTests",
            dependencies: [
                "Resources",
                "SemanticModel",
            ],
            path: "Tests/ResourcesTests"
        ),
        .testTarget(
            name: "AccessibilityTests",
            dependencies: [
                "Accessibility",
                "SemanticModel",
            ],
            path: "Tests/AccessibilityTests"
        ),
        .testTarget(
            name: "SRUITests",
            dependencies: [
                "TransportSSH",
                "Protocol",
                "SemanticModel",
                "Session",
                "RendererAppKit",
                "Collections",
                "Text",
                "Terminal",
                "Resources",
                "Accessibility",
            ],
            path: "Tests/SRUITests"
        ),
    ]
)
