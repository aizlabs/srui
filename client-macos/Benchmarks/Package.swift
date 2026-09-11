// swift-tools-version: 6.0
import PackageDescription

/// Task 34 benchmark executable, isolated from the production SRUIClient build graph.
let package = Package(
    name: "SRUIBenchmarks",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "BenchmarkDriver", targets: ["BenchmarkDriver"]),
    ],
    dependencies: [
        .package(name: "SRUIClient", path: ".."),
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.28.2"),
    ],
    targets: [
        .executableTarget(
            name: "BenchmarkDriver",
            dependencies: [
                .product(name: "Protocol", package: "SRUIClient"),
                .product(name: "SemanticModel", package: "SRUIClient"),
                .product(name: "Session", package: "SRUIClient"),
                .product(name: "TransportSSH", package: "SRUIClient"),
                .product(name: "RendererAppKit", package: "SRUIClient"),
                .product(name: "Resources", package: "SRUIClient"),
                .product(name: "Terminal", package: "SRUIClient"),
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ],
            path: ".",
            exclude: ["Tests"]
        ),
        .testTarget(
            name: "BenchmarkDriverTests",
            dependencies: ["BenchmarkDriver"],
            path: "Tests"
        ),
    ]
)
