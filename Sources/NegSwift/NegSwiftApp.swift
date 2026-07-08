import AppKit
import RawDecodeKit
import SwiftUI

@main
struct NegSwiftApp: App {
    init() {
        // Running unbundled via `swift run` needs an explicit activation policy
        // for the window to appear and take focus.
        NSApplication.shared.setActivationPolicy(.regular)
        DispatchQueue.main.async { NSApplication.shared.activate(ignoringOtherApps: true) }
    }

    var body: some Scene {
        WindowGroup("NegSwift") {
            ContentView()
        }
    }
}

/// Phase 1 stub: open a RAW file, show the linear sensor decode (dark, orange-mask
/// negative). The conversion pipeline replaces this in Phase 3+.
struct ContentView: View {
    @State private var image: CGImage?
    @State private var status = "Open a RAW negative to test the linear decode."

    var body: some View {
        VStack(spacing: 12) {
            if let image {
                Image(decorative: image, scale: 1.0)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
            } else {
                Text(status).foregroundStyle(.secondary)
            }
            Button("Open RAW…") { openRaw() }
                .padding(.bottom, 12)
        }
        .frame(minWidth: 900, minHeight: 600)
    }

    private func openRaw() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        status = "Decoding…"
        image = nil
        Task.detached {
            do {
                let img = try RawDecoder().decode(url: url, quality: .preview, maxLongEdge: 1536)
                // Quick 8-bit sRGB-ish preview of the raw linear data (temporary).
                var bytes = [UInt8](repeating: 0, count: img.width * img.height * 3)
                for i in 0..<bytes.count {
                    bytes[i] = UInt8(max(0, min(255, pow(Double(img.pixels[i]), 1 / 2.2) * 255)))
                }
                let data = bytes.withUnsafeBufferPointer { Data(buffer: $0) }
                let cg = CGDataProvider(data: data as CFData).flatMap {
                    CGImage(
                        width: img.width, height: img.height, bitsPerComponent: 8, bitsPerPixel: 24,
                        bytesPerRow: img.width * 3, space: CGColorSpaceCreateDeviceRGB(),
                        bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.none.rawValue),
                        provider: $0, decode: nil, shouldInterpolate: true, intent: .defaultIntent
                    )
                }
                await MainActor.run { image = cg }
            } catch {
                await MainActor.run { status = "Decode failed: \(error)" }
            }
        }
    }
}
