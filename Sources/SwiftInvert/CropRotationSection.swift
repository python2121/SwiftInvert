import SwiftUI

/// Grid overlay styles drawn over the image (display-only, app-wide setting —
/// like NegPy's crop guides, not part of the per-image edit).
enum GridLineType: String, CaseIterable, Identifiable {
    case thirds
    case phi
    case grid
    case diagonals

    var id: String { rawValue }
    var label: String {
        switch self {
        case .thirds: return "Rule of thirds"
        case .phi: return "Phi (golden ratio)"
        case .grid: return "Grid"
        case .diagonals: return "Diagonals"
        }
    }
}

/// Crop tool, straighten (fine rotation) and grid lines — its own collapsible
/// section above History.
struct CropRotationSection: View {
    @Bindable var model: AppModel
    @AppStorage("cropRotationCollapsed") private var collapsed = false
    @AppStorage("showGridLines") private var showGridLines = false
    @AppStorage("gridLineType") private var gridLineType = GridLineType.thirds.rawValue

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if !collapsed {
                content
            }
        }
        .padding(.vertical, 8)
    }

    private var header: some View {
        Button {
            collapsed.toggle()
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .rotationEffect(.degrees(collapsed ? 0 : 90))
                    .foregroundStyle(.secondary)
                Text("Crop & Rotation").font(.headline)
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 12)
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Button {
                    model.toolMode = model.toolMode == .crop ? .none : .crop
                } label: {
                    Label("Crop", systemImage: "crop")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .tint(model.toolMode == .crop ? Color.accentColor : nil)
                if model.settings.cropRect != nil {
                    Button {
                        model.pendingHistoryLabel = "Crop cleared"
                        model.settings.cropRect = nil
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                    }
                    .buttonStyle(.borderless)
                    .help("Clear crop")
                }
            }

            LabeledSlider(
                label: "Straighten", value: $model.settings.fineRotation,
                range: -45...45, format: "%.1f°", defaultValue: 0)

            Toggle("Show grid lines", isOn: $showGridLines)
            if showGridLines {
                Picker("", selection: $gridLineType) {
                    ForEach(GridLineType.allCases) { type in
                        Text(type.label).tag(type.rawValue)
                    }
                }
                .labelsHidden()
            }
        }
        .padding(.horizontal, 12)
        .disabled(model.selection == nil)
    }
}

/// The grid drawn over the displayed image (transforms with zoom/pan since it
/// shares the image's container).
struct GridOverlay: View {
    let type: GridLineType

    var body: some View {
        Canvas { context, size in
            let w = size.width, h = size.height
            var path = Path()
            func vline(_ x: CGFloat) {
                path.move(to: CGPoint(x: x, y: 0))
                path.addLine(to: CGPoint(x: x, y: h))
            }
            func hline(_ y: CGFloat) {
                path.move(to: CGPoint(x: 0, y: y))
                path.addLine(to: CGPoint(x: w, y: y))
            }
            switch type {
            case .thirds:
                vline(w / 3); vline(2 * w / 3)
                hline(h / 3); hline(2 * h / 3)
            case .phi:
                let inv: CGFloat = 0.38196601  // 1 − 1/φ
                vline(w * inv); vline(w * (1 - inv))
                hline(h * inv); hline(h * (1 - inv))
            case .grid:
                for i in 1..<8 {
                    vline(w * CGFloat(i) / 8)
                    hline(h * CGFloat(i) / 8)
                }
            case .diagonals:
                path.move(to: .zero)
                path.addLine(to: CGPoint(x: w, y: h))
                path.move(to: CGPoint(x: w, y: 0))
                path.addLine(to: CGPoint(x: 0, y: h))
            }
            context.stroke(path, with: .color(.white.opacity(0.55)), lineWidth: 1)
            context.stroke(path, with: .color(.black.opacity(0.25)), lineWidth: 2.5)
        }
        .allowsHitTesting(false)
    }
}
