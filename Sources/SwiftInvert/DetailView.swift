import NegativeKit
import SwiftUI
import simd

struct DetailView: View {
    @Bindable var model: AppModel

    @AppStorage("showGridLines") private var showGridLines = false
    @AppStorage("gridLineType") private var gridLineType = GridLineType.thirds.rawValue

    @State private var zoom: CGFloat = 1
    /// Desired crop box while in Crop & Straighten mode (rotated-space px;
    /// nil = follow the committed crop). Committed back on mode exit.
    @State private var cropBox: CropBoxValue?
    @State private var baseZoom: CGFloat = 1
    @State private var pan: CGSize = .zero
    @State private var basePan: CGSize = .zero
    /// Spot-densitometer hover state. Held here but never READ from this
    /// body — only `DensitometerReadout` reads it, so metering the pointer
    /// doesn't re-render the canvas.
    @State private var densitometer = DensitometerState()

    var body: some View {
        VStack(spacing: 0) {
            canvas
            Divider()
            controlBar
        }
        // Grab the rendered bytes once per render; the probe reads them.
        .onChange(of: model.displayImage) { _, new in densitometer.adopt(new) }
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
                Button {
                    model.hqPreview.toggle()
                } label: {
                    Text("HQ")
                        .font(.system(size: 10, weight: .semibold))
                        .padding(.horizontal, 5)
                        .padding(.vertical, 2)
                        .background(
                            RoundedRectangle(cornerRadius: 4)
                                .fill(model.hqPreview ? Color.accentColor.opacity(0.3) : Color.primary.opacity(0.06)))
                        .overlay(
                            RoundedRectangle(cornerRadius: 4)
                                .strokeBorder(
                                    model.hqPreview ? Color.accentColor : Color.secondary.opacity(0.4),
                                    lineWidth: 1))
                }
                .help("Preview at full source resolution (slower); off = 1536px proxy")
            }
            .disabled(model.selection == nil)

            Spacer()

            if model.selection != nil {
                DensitometerReadout(state: densitometer)
            }

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
            // Straighten preview state (see the Image below): target angle,
            // its delta from the baked angle, and — for 0°-base previews —
            // the inscribed-rect clip window replacing the plain fitted frame.
            let target = model.straightenDragValue ?? model.settings.fineRotation
            let radians = target * .pi / 180
            let delta = target - model.displayedFineRotation
            let cropMode = model.toolMode == .crop
            let zeroBase = !cropMode && model.displayedFineRotation == 0 && abs(delta) > 1e-9
            // Meter only the plain presentation: the tool modes and the
            // straighten preview show transient geometry (uncropped frames,
            // 0°-base re-bakes) whose pixels aren't the ones an edit is being
            // judged against.
            let probeActive =
                !cropMode && !zeroBase && model.toolMode == .none
                && model.straightenDragValue == nil
            let inscribed = RGBImage.inscribedRectSize(
                width: Double(image.width), height: Double(image.height), radians: radians)
            // Crop mode fits the ROTATED frame's bounding box so the whole
            // image stays visible while it turns behind the crop box.
            let framePx: SIMD2<Double> = model.frameSize == .zero
                ? SIMD2(Double(image.width), Double(image.height))
                : SIMD2(Double(model.frameSize.width), Double(model.frameSize.height))
            let bbox = CGSize(
                width: framePx.x * abs(cos(radians)) + framePx.y * abs(sin(radians)),
                height: framePx.x * abs(sin(radians)) + framePx.y * abs(cos(radians)))
            let window =
                cropMode
                ? fittedRect(imageSize: bbox, in: geo.size)
                : (zeroBase
                    ? fittedRect(
                        imageSize: CGSize(width: inscribed.width, height: inscribed.height),
                        in: geo.size)
                    : fitted)
            let cropScale = window.width / bbox.width
            let frameScale = window.width / CGFloat(inscribed.width)
            let imageFrame: CGSize? =
                cropMode
                ? CGSize(width: framePx.x * cropScale, height: framePx.y * cropScale)
                : (zeroBase
                    ? CGSize(
                        width: CGFloat(image.width) * frameScale,
                        height: CGFloat(image.height) * frameScale)
                    : nil)
            ZStack {
                ZStack {
                    // Straighten preview: the display rotates by the difference
                    // between the target angle (drag value or committed setting)
                    // and the angle the shown image was baked with — live during
                    // the drag, and held through the post-release re-bake so the
                    // image doesn't snap back while analysis re-runs.
                    // 0°-base previews get the EXACT inscribed presentation:
                    // the clip window takes the target angle's inscribed-rect
                    // aspect, and the full frame is scaled so that rect fills
                    // it precisely — pixel-identical to a baked render at the
                    // same angle, so press-swap, drag and re-bake all align
                    // (a frame-cover scale shows a different magnification
                    // than the baked inscribed crop). Cover-scale remains for
                    // the cache-miss fallback (base still baked at an angle).
                    Image(decorative: image, scale: 1.0)
                        .resizable()
                        .interpolation(.high)
                        .frame(width: imageFrame?.width, height: imageFrame?.height)
                        // Above the frame, below the transforms: local coords
                        // here span the displayed bitmap exactly, and SwiftUI
                        // inverse-maps the enclosing zoom/pan itself — so the
                        // probe stays correct at any magnification without
                        // duplicating the geometry (hover isn't a gesture, so
                        // this doesn't touch pan/magnify/double-tap).
                        .onContinuousHover(coordinateSpace: .local) { phase in
                            guard probeActive, case .active(let p) = phase else {
                                densitometer.clear()
                                return
                            }
                            densitometer.probe(u: p.x / window.width, v: p.y / window.height)
                        }
                        .rotationEffect(.degrees(cropMode || zeroBase ? target : delta))
                        .scaleEffect(
                            cropMode || zeroBase || delta == 0
                                ? 1 : coverScale(image: image, degrees: delta))
                    // Grid: on while its box is checked OR while straightening —
                    // and it stays axis-aligned (that's what you level against).
                    if model.toolMode == .none, let type = GridLineType(rawValue: gridLineType),
                        showGridLines || model.straightenDragValue != nil
                    {
                        GridOverlay(type: type)
                    }
                }
                .frame(width: window.width, height: window.height)
                .clipped()
                .scaleEffect(cropMode ? 1 : zoom)
                .offset(cropMode ? .zero : pan)
                .position(x: geo.size.width / 2, y: geo.size.height / 2)

                if cropMode {
                    CropBoxOverlay(
                        box: $cropBox, frame: framePx, radians: radians, scale: cropScale,
                        committed: model.settings.cropRect,
                        committedRadians: model.settings.fineRotation * .pi / 180)
                        .frame(width: window.width, height: window.height)
                        .position(x: geo.size.width / 2, y: geo.size.height / 2)
                } else if model.toolMode != .none {
                    SelectionOverlay(frame: fitted, existing: existingRect) { rect in
                        model.commitSelection(rect)
                    }
                }
            }
            .clipped()
            .contentShape(Rectangle())
            // Two-finger trackpad scroll pans when zoomed in (Preview-style);
            // same constraints as the drag gesture.
            .background(
                ScrollWheelCatcher { dx, dy in
                    guard model.toolMode == .none, zoom > 1 else { return }
                    pan.width += dx
                    pan.height += dy
                    basePan = pan
                })
            .gesture(zoomAndPanGestures, isEnabled: model.toolMode == .none)
            .onTapGesture(count: 2) {
                withAnimation(.easeOut(duration: 0.15)) { resetZoom() }
            }
            .onChange(of: model.toolMode) { old, new in
                if old == .crop {
                    if model.cropModeCancelled {
                        model.cropModeCancelled = false
                        cropBox = nil
                    } else {
                        commitCrop()
                    }
                }
                if new == .crop { cropBox = nil }
            }
            // First straighten drag inside crop mode freezes the box on
            // screen (Lightroom behavior: the box holds, the image turns).
            .onChange(of: model.straightenDragValue) { _, value in
                if model.toolMode == .crop, value != nil, cropBox == nil {
                    cropBox = CropBoxOverlay.defaultBox(
                        committed: model.settings.cropRect,
                        committedRadians: model.settings.fineRotation * .pi / 180,
                        frame: SIMD2(Double(model.frameSize.width), Double(model.frameSize.height)),
                        radians: (model.straightenDragValue ?? model.settings.fineRotation) * .pi / 180)
                }
            }
            // Clear Crop (menu/sidebar) while in the mode resets the box.
            .onChange(of: model.settings.cropRect) { _, rect in
                if model.toolMode == .crop, rect == nil { cropBox = nil }
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

    /// Commit the crop box on leaving Crop & Straighten mode: constrain the
    /// desired box to the committed angle, convert to a NormalizedRect over
    /// the inscribed rect, and treat a (nearly) full-frame box as "no crop".
    private func commitCrop() {
        defer { cropBox = nil }
        guard let desired = cropBox, model.frameSize != .zero else { return }
        let framePx = SIMD2(Double(model.frameSize.width), Double(model.frameSize.height))
        let radians = model.settings.fineRotation * .pi / 180
        let (center, halfExtents) = CropGeometry.constrain(
            center: desired.center, halfExtents: desired.halfExtents, radians: radians,
            frame: framePx)
        let inscribed = CropGeometry.inscribedSize(frame: framePx, radians: radians)
        let isFullFrame =
            simd_length(center) < 0.005 * inscribed.x
            && abs(halfExtents.x * 2 - inscribed.x) < 0.01 * inscribed.x
            && abs(halfExtents.y * 2 - inscribed.y) < 0.01 * inscribed.y
        if isFullFrame {
            if model.settings.cropRect != nil {
                model.pendingHistoryLabel = "Crop cleared"
                model.settings.cropRect = nil
            }
        } else {
            let rect = CropGeometry.normalizedRect(
                center: center, halfExtents: halfExtents, frame: framePx, radians: radians)
            if model.settings.cropRect != rect {
                model.pendingHistoryLabel = "Crop"
                model.settings.cropRect = rect
            }
        }
    }

    /// Scale that keeps the rotated preview covering the frame (the display
    /// stand-in for the inscribed-rect crop the real bake applies).
    private func coverScale(image: CGImage, degrees: Double) -> CGFloat {
        let w = Double(image.width), h = Double(image.height)
        let inscribed = RGBImage.inscribedRectSize(
            width: w, height: h, radians: degrees * .pi / 180)
        return CGFloat(max(w / max(inscribed.width, 1), h / max(inscribed.height, 1)))
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
