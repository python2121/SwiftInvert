import NegativeKit
import SwiftUI

struct DetailView: View {
    @Bindable var model: AppModel

    @AppStorage("showGridLines") private var showGridLines = false
    @AppStorage("gridLineType") private var gridLineType = GridLineType.thirds.rawValue

    @State private var zoom: CGFloat = 1
    @State private var baseZoom: CGFloat = 1
    @State private var pan: CGSize = .zero
    @State private var basePan: CGSize = .zero

    var body: some View {
        VStack(spacing: 0) {
            canvas
            Divider()
            controlBar
        }
    }

    private var canvas: some View {
        ZStack {
            model.canvasColor.color
            if let image = model.displayImage {
                imageCanvas(image)
            } else if let status = model.statusMessage, model.selection != nil {
                ContentUnavailableView {
                    Label("Couldn't develop this image", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(status)
                } actions: {
                    Button("Choose Folder Again…") { model.chooseFolder() }
                }
            } else if model.selection != nil {
                ProgressView("Developing…")
            } else {
                ContentUnavailableView(
                    "No image selected", systemImage: "film",
                    description: Text("Select a negative from the library."))
            }
        }
        .overlay(alignment: .bottomLeading) {
            VStack(alignment: .leading, spacing: 6) {
                if let progress = model.exportProgress {
                    HStack(spacing: 8) {
                        ProgressView(value: Double(progress.done), total: Double(max(progress.total, 1)))
                            .frame(width: 130)
                        VStack(alignment: .leading, spacing: 1) {
                            Text("Exporting \(progress.done + 1) of \(progress.total)")
                                .font(.caption)
                            Text(progress.currentName)
                                .font(.system(size: 9))
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                        Button {
                            model.cancelExport()
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                        }
                        .buttonStyle(.borderless)
                        .help("Cancel export")
                    }
                    .padding(8)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
                }
                if model.isAnalyzing {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Analyzing…").font(.caption)
                    }
                    .padding(8)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
                }
            }
            .padding(10)
        }
        .onChange(of: model.toolMode) { _, mode in
            // Selection drags are mapped on the fitted frame; reset zoom first.
            if mode != .none { resetZoom() }
        }
        .onChange(of: model.selection) { _, _ in resetZoom() }
    }

    /// Rotate/flip + canvas color, under the image.
    private var controlBar: some View {
        HStack(spacing: 14) {
            Group {
                Button {
                    model.rotateCounterclockwise()
                } label: {
                    Image(systemName: "rotate.left")
                }
                .help("Rotate counterclockwise")
                Button {
                    model.rotateClockwise()
                } label: {
                    Image(systemName: "rotate.right")
                }
                .help("Rotate clockwise")
                Button {
                    model.flipHorizontal()
                } label: {
                    Image(systemName: "arrow.left.and.right.righttriangle.left.righttriangle.right")
                }
                .help("Flip horizontally")
            }
            .disabled(model.selection == nil)

            Spacer()

            Text("Canvas").font(.caption).foregroundStyle(.secondary)
            ForEach(AppModel.CanvasColor.allCases) { option in
                Button {
                    model.canvasColor = option
                } label: {
                    Circle()
                        .fill(option.color)
                        .frame(width: 16, height: 16)
                        .overlay {
                            Circle().strokeBorder(
                                model.canvasColor == option ? Color.accentColor : Color.secondary.opacity(0.4),
                                lineWidth: model.canvasColor == option ? 2 : 1)
                        }
                }
                .buttonStyle(.plain)
                .help(option.label)
            }
        }
        .buttonStyle(.borderless)
        .imageScale(.medium)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }

    private func resetZoom() {
        zoom = 1
        baseZoom = 1
        pan = .zero
        basePan = .zero
    }

    /// Aspect-fit frame of the image within the container (12pt inset).
    private func fittedRect(imageSize: CGSize, in container: CGSize) -> CGRect {
        let inset: CGFloat = 12
        let avail = CGSize(width: max(container.width - 2 * inset, 1), height: max(container.height - 2 * inset, 1))
        let scale = min(avail.width / imageSize.width, avail.height / imageSize.height)
        let size = CGSize(width: imageSize.width * scale, height: imageSize.height * scale)
        return CGRect(
            x: (container.width - size.width) / 2, y: (container.height - size.height) / 2,
            width: size.width, height: size.height)
    }

    @ViewBuilder
    private func imageCanvas(_ image: CGImage) -> some View {
        GeometryReader { geo in
            let fitted = fittedRect(
                imageSize: CGSize(width: image.width, height: image.height), in: geo.size)
            ZStack {
                ZStack {
                    Image(decorative: image, scale: 1.0)
                        .resizable()
                        .interpolation(.high)
                    if showGridLines, model.toolMode == .none,
                        let type = GridLineType(rawValue: gridLineType)
                    {
                        GridOverlay(type: type)
                    }
                }
                .frame(width: fitted.width, height: fitted.height)
                .scaleEffect(zoom)
                .offset(pan)
                .position(x: geo.size.width / 2, y: geo.size.height / 2)

                if model.toolMode != .none {
                    SelectionOverlay(frame: fitted, existing: existingRect) { rect in
                        model.commitSelection(rect)
                    }
                }
            }
            .clipped()
            .contentShape(Rectangle())
            .gesture(zoomAndPanGestures, isEnabled: model.toolMode == .none)
            .onTapGesture(count: 2) {
                withAnimation(.easeOut(duration: 0.15)) { resetZoom() }
            }
            .overlay(alignment: .top) {
                if model.showingBaseline {
                    Text("Original conversion")
                        .font(.caption)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(.black.opacity(0.65), in: Capsule())
                        .foregroundStyle(.white)
                        .padding(.top, 10)
                }
            }
        }
    }

    private var existingRect: NormalizedRect? {
        switch model.toolMode {
        case .analysisRegion:
            // No custom region → show the default centered 80% the meters use.
            let b = ExposureKernel.defaultAnalysisBuffer
            return model.settings.analysisRect
                ?? NormalizedRect(x: b, y: b, width: 1 - 2 * b, height: 1 - 2 * b)
        case .crop: return model.settings.cropRect
        case .none: return nil
        }
    }

    private var zoomAndPanGestures: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                // 0.5 lets you pull back well past fit; 8x for pixel peeping.
                zoom = min(max(baseZoom * value.magnification, 0.5), 8)
            }
            .onEnded { _ in
                baseZoom = zoom
                if zoom <= 1 { withAnimation(.easeOut(duration: 0.15)) { pan = .zero; basePan = .zero } }
            }
            .simultaneously(
                with: DragGesture()
                    .onChanged { value in
                        guard zoom > 1 else { return }
                        pan = CGSize(
                            width: basePan.width + value.translation.width,
                            height: basePan.height + value.translation.height)
                    }
                    .onEnded { _ in basePan = pan }
            )
    }
}

/// Drag-to-draw rect selection over the fitted image frame. Shows the currently
/// committed rect (accent) and the in-progress drag (white).
struct SelectionOverlay: View {
    let frame: CGRect
    let existing: NormalizedRect?
    let onCommit: (NormalizedRect) -> Void

    @State private var dragStart: CGPoint?
    @State private var dragCurrent: CGPoint?

    var body: some View {
        ZStack {
            Color.black.opacity(0.25)  // dims the frame so the selection pops
            if let rect = activeRect {
                let r = screenRect(rect)
                Rectangle()
                    .path(in: r)
                    .fill(Color.white.opacity(0.12))
                Rectangle()
                    .path(in: r)
                    .stroke(dragStart != nil ? Color.white : Color.accentColor, lineWidth: 1.5)
            }
            Text("Drag to select — Esc to cancel")
                .font(.caption)
                .padding(6)
                .background(.black.opacity(0.6), in: RoundedRectangle(cornerRadius: 5))
                .foregroundStyle(.white)
                .position(x: frame.midX, y: frame.minY + 18)
        }
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 3)
                .onChanged { value in
                    if dragStart == nil { dragStart = value.startLocation }
                    dragCurrent = value.location
                }
                .onEnded { _ in
                    defer {
                        dragStart = nil
                        dragCurrent = nil
                    }
                    guard let rect = draggedRect, rect.width > 0.01, rect.height > 0.01 else { return }
                    onCommit(rect)
                }
        )
    }

    private var activeRect: NormalizedRect? { draggedRect ?? existing }

    private var draggedRect: NormalizedRect? {
        guard let a = dragStart, let b = dragCurrent else { return nil }
        return NormalizedRect(from: normalized(a), to: normalized(b))
    }

    private func normalized(_ p: CGPoint) -> (x: Double, y: Double) {
        (
            x: Double(min(max((p.x - frame.minX) / frame.width, 0), 1)),
            y: Double(min(max((p.y - frame.minY) / frame.height, 0), 1))
        )
    }

    private func screenRect(_ r: NormalizedRect) -> CGRect {
        CGRect(
            x: frame.minX + r.x * frame.width,
            y: frame.minY + r.y * frame.height,
            width: r.width * frame.width,
            height: r.height * frame.height)
    }
}
