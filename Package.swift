// swift-tools-version: 6.0
import Foundation
import PackageDescription

let v5 = [SwiftSetting.swiftLanguageMode(.v5)]

#if os(macOS)
// LibRaw's .pc files put `-fopenmp` in their Libs line (Homebrew as
// `-Xpreprocessor -fopenmp`, apt's 0.21 as bare `-fopenmp`); SwiftPM
// refuses to forward it ("prohibited flag(s)" twice per build) and it does
// nothing for us anyway — OpenMP is internal to the shared library, which
// carries its own libgomp/liblcms2/libstdc++ NEEDED entries. So NO platform
// consults the .pc: the modulemap's `link "raw_r"` picks the library and
// the shim's `<libraw/libraw.h>` include resolves from the default search
// path on Linux (/usr/include). macOS still needs the Homebrew include/lib
// dirs passed explicitly, on every target that (transitively) imports
// RawDecodeKit: explicit-modules builds rebuild the CLibRaw clang module
// per importer, so the include flag rides all of them (what pkg-config
// would have propagated).
let brewPrefix = ProcessInfo.processInfo.environment["HOMEBREW_PREFIX"]
    ?? (FileManager.default.fileExists(atPath: "/opt/homebrew/include/libraw") ? "/opt/homebrew" : "/usr/local")
let libRaw: Target = .systemLibrary(name: "CLibRaw", providers: [.brew(["libraw"])])
let libRawSwift: [SwiftSetting] = [.unsafeFlags(["-Xcc", "-I\(brewPrefix)/include"])]
let libRawLinker: [LinkerSetting] = [.unsafeFlags(["-L\(brewPrefix)/lib"])]
#else
let libRaw: Target = .systemLibrary(name: "CLibRaw", providers: [.apt(["libraw-dev"])])
let libRawSwift: [SwiftSetting] = []
let libRawLinker: [LinkerSetting] = []
#endif

// Portable core: builds on macOS and Linux (the Qt frontend consumes these).
var targets: [Target] = [
    libRaw,
    // Pure conversion kernel: analysis, metering, curve parameters. No UI, no Metal.
    .target(name: "NegativeKit", swiftSettings: v5),
    .target(
        name: "RawDecodeKit", dependencies: ["CLibRaw", "NegativeKit"],
        swiftSettings: v5 + libRawSwift, linkerSettings: libRawLinker),
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
        swiftSettings: v5 + libRawSwift
    ),
    .executableTarget(
        name: "SwiftInvert",
        dependencies: ["RawDecodeKit", "NegativeKit", "MetalRenderKit"],
        resources: [.copy("Resources")],
        swiftSettings: v5 + libRawSwift
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
        swiftSettings: v5 + libRawSwift
    ),
]
#endif

let package = Package(
    name: "SwiftInvert",
    // The Homebrew dylibs the .app bundles are built for macOS 26, so an
    // older declared target only earns ld's "built for newer version"
    // warning on every link. Packaging/Info.plist's LSMinimumSystemVersion
    // mirrors this.
    platforms: [.macOS("26.0")],  // string form: `.v26` needs tools-version 6.2
    products: products,
    targets: targets
)
