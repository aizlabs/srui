// swift-tools-version: 6.0
import PackageDescription

/// Portable logical-channel scheduler (§18.2, §19.2).
///
/// Nested as its own package so Linux CI can `swift test` this module without
/// compiling TransportSSH, AppKit, or the rest of `client-macos`.
let package = Package(
    name: "LogicalChannelScheduling",
    products: [
        .library(
            name: "LogicalChannelScheduling",
            targets: ["LogicalChannelScheduling"]
        ),
    ],
    targets: [
        .target(
            name: "LogicalChannelScheduling"
        ),
        .testTarget(
            name: "LogicalChannelSchedulingTests",
            dependencies: ["LogicalChannelScheduling"]
        ),
    ]
)
