// swift-tools-version: 6.0
import PackageDescription

let v5 = [SwiftSetting.swiftLanguageMode(.v5)]

// Portable core: builds on macOS and Linux (the Qt frontend consumes these).
var targets: [Target] = [
    .systemLibrary(
        name: "CLibRaw",
        pkgConfig: "libraw_r",
        providers: [.brew(["libraw"]), .apt(["libraw-dev"])]
    ),
    // Pure conversion kernel: analysis, metering, curve parameters. No UI, no Metal.
    .target(name: "NegativeKit", swiftSettings: v5),
    .target(name: "RawDecodeKit", dependencies: ["CLibRaw", "NegativeKit"], swiftSettings: v5),
    .testTarget(
        name: "NegativeKitTests",
        dependencies: ["NegativeKit"],
        swiftSettings: v5
    ),
]

// negcli builds everywhere; only the macOS build links the Metal renderer.
// On Linux the GPU path is VulkanRenderKit (compute mirror of the Metal
// chain; ReferenceCurve remains the no-GPU fallback). Declared per-branch
// because SwiftPM validates conditional dependency names even when the
// condition is false.
// The Qt frontend consumes the core through this C-ABI dylib (CoreBridge's
// @_cdecl surface; qt/swiftinvert_core.h is the matching header).
// Products are explicit because of the dylib; executables must then be
// listed per-platform too (an explicit list suppresses the implicit ones,
// and `swift run` / `--product` only see listed products).
var products: [Product] = []
#if os(macOS)
products += [
    .executable(name: "SwiftInvert", targets: ["SwiftInvert"]),
    .executable(name: "negcli", targets: ["negcli"]),
]
#else
products += [
    .library(name: "SwiftInvertCore", type: .dynamic, targets: ["CoreBridge"]),
    .executable(name: "negcli", targets: ["negcli"]),
]
#endif
#if !os(macOS)
targets += [
    .systemLibrary(
        name: "CVulkan",
        pkgConfig: "vulkan",
        providers: [.apt(["libvulkan-dev"])]
    ),
    .target(
        name: "CoreBridge",
        dependencies: ["NegativeKit", "RawDecodeKit", "VulkanRenderKit"],
        swiftSettings: v5
    ),
    .target(
        name: "VulkanRenderKit",
        dependencies: ["CVulkan", "NegativeKit"],
        resources: [.copy("Shaders")],
        swiftSettings: v5
    ),
    .executableTarget(
        name: "negcli",
        dependencies: ["RawDecodeKit", "NegativeKit", "VulkanRenderKit"],
        swiftSettings: v5
    ),
    .testTarget(
        name: "VulkanRenderKitTests",
        dependencies: ["VulkanRenderKit", "NegativeKit"],
        swiftSettings: v5
    ),
]
#endif

// Package.swift executes on the build host, so this gates by where the build
// runs — exactly right for Metal/SwiftUI, which only exist there.
#if os(macOS)
targets += [
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
        name: "SwiftInvert",
        dependencies: ["RawDecodeKit", "NegativeKit", "MetalRenderKit"],
        resources: [.copy("Resources")],
        swiftSettings: v5
    ),
    .testTarget(
        name: "MetalRenderKitTests",
        dependencies: ["MetalRenderKit", "NegativeKit"],
        swiftSettings: v5
    ),
    // App-layer logic (SwiftPM can test @main executables since 5.5):
    // history labels, sidecar IO, export options, densitometer probe.
    .testTarget(
        name: "SwiftInvertTests",
        dependencies: ["SwiftInvert", "NegativeKit"],
        swiftSettings: v5
    ),
]
#endif

let package = Package(
    name: "SwiftInvert",
    platforms: [.macOS(.v14)],
    products: products,
    targets: targets
)
