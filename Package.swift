// swift-tools-version: 6.0
import PackageDescription

let v5 = [SwiftSetting.swiftLanguageMode(.v5)]

let package = Package(
    name: "NegSwift",
    platforms: [.macOS(.v14)],
    targets: [
        .systemLibrary(
            name: "CLibRaw",
            pkgConfig: "libraw_r",
            providers: [.brew(["libraw"])]
        ),
        // Pure conversion kernel: analysis, metering, curve parameters. No UI, no Metal.
        .target(name: "NegativeKit", swiftSettings: v5),
        .target(name: "RawDecodeKit", dependencies: ["CLibRaw", "NegativeKit"], swiftSettings: v5),
        .target(
            name: "MetalRenderKit",
            dependencies: ["NegativeKit"],
            resources: [.copy("Shaders")],
            swiftSettings: v5
        ),
        .executableTarget(
            name: "negcli",
            dependencies: ["RawDecodeKit", "NegativeKit", "MetalRenderKit"],
            swiftSettings: v5
        ),
        .executableTarget(
            name: "NegSwift",
            dependencies: ["RawDecodeKit", "NegativeKit", "MetalRenderKit"],
            swiftSettings: v5
        ),
        .testTarget(
            name: "NegativeKitTests",
            dependencies: ["NegativeKit"],
            swiftSettings: v5
        ),
        .testTarget(
            name: "MetalRenderKitTests",
            dependencies: ["MetalRenderKit", "NegativeKit"],
            swiftSettings: v5
        ),
    ]
)
