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
            name: "Session",
            dependencies: [
                "TransportSSH",
                "Protocol",
                "SemanticModel",
                "RendererAppKit",
            ],
            path: "Session"
        ),
        .target(
            name: "RendererAppKit",
            dependencies: [
                "SemanticModel",
            ],
            path: "RendererAppKit"
        ),
        .executableTarget(
            name: "RendererDemoApp",
            dependencies: [
                "Session",
                "RendererAppKit",
                "TransportSSH",
                "Protocol",
                "SemanticModel",
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
            name: "Resources",
            path: "Resources"
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
            ],
            path: "Tests/RendererAppKitTests"
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
