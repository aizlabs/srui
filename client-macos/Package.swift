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
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.28.2"),
    ],
    targets: [
        .target(
            name: "TransportSSH",
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
            ],
            path: "RendererAppKit"
        ),
        .target(
            name: "Session",
            dependencies: [
                "TransportSSH",
                "Protocol",
                "SemanticModel",
                "RendererAppKit",
                "Resources",
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
            path: "Collections"
        ),
        .target(
            name: "Text",
            path: "Text"
        ),
        .target(
            name: "Terminal",
            path: "Terminal"
        ),
        .target(
            name: "Accessibility",
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
            ],
            path: "Tests/RendererAppKitTests"
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
