import AppKit
import SwiftUI

/// The library: a VSCode-style collapsible folder tree whose leaves are
/// thumbnail strips (previews + index badges, no filenames). Plain panel
/// styling to match the adjustments sidebar (no vibrancy overlay).
struct LibraryView: View {
    @Bindable var model: AppModel
    var onToggleVisibility: () -> Void = {}

    private let columns = [GridItem(.adaptive(minimum: 120, maximum: 180), spacing: 8)]

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var header: some View {
        HStack {
            Text(model.folderURL?.lastPathComponent ?? "Library")
                .font(.headline)
                .lineLimit(1)
                .truncationMode(.middle)
                .help(model.folderURL?.path ?? "")
            if model.isScanning {
                ProgressView().controlSize(.small)
            }
            Spacer()
            Button {
                model.chooseFolder()
            } label: {
                Image(systemName: "folder")
            }
            .buttonStyle(.borderless)
            .help("Choose library folder…")
            Button {
                onToggleVisibility()
            } label: {
                Image(systemName: "sidebar.left")
            }
            .buttonStyle(.borderless)
            .help("Hide library")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    @ViewBuilder
    private var content: some View {
        if model.folderURL == nil {
            ContentUnavailableView {
                Label("No folder selected", systemImage: "folder")
            } description: {
                Text("Choose a folder — subfolders become collapsible film strips.")
            } actions: {
                Button("Choose Folder…") { model.chooseFolder() }
                    .help("Pick the folder of RAW negatives to browse")
            }
        } else if let tree = model.folderTree {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        FolderSection(model: model, node: tree, depth: 0, isRoot: true, columns: columns)
                    }
                    .padding(8)
                }
                // Keyboard navigation must keep the selected cell in view
                // (cells take their ForEach identity, the file URL).
                .onChange(of: model.selection) { _, selection in
                    if let selection {
                        proxy.scrollTo(selection)
                    }
                }
            }
        } else if model.isScanning {
            VStack {
                Spacer()
                ProgressView("Scanning…")
                Spacer()
            }
            .frame(maxWidth: .infinity)
        } else {
            ContentUnavailableView(
                "No RAW files", systemImage: "photo.on.rectangle.angled",
                description: Text("No supported camera RAW files in this folder tree."))
        }
    }
}

/// One folder in the tree: a collapsible header row plus its thumbnail strip
/// and child folders (recursive).
struct FolderSection: View {
    var model: AppModel
    let node: AppModel.FolderNode
    let depth: Int
    let isRoot: Bool
    let columns: [GridItem]

    private var isCollapsed: Bool { model.collapsedFolders.contains(node.id) }
    private var indent: CGFloat { CGFloat(depth) * 14 }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !isRoot {
                folderRow
            }
            if isRoot || !isCollapsed {
                if !node.files.isEmpty {
                    LazyVGrid(columns: columns, spacing: 8) {
                        ForEach(node.files, id: \.self) { url in
                            LibraryCell(model: model, url: url)
                        }
                    }
                    .padding(.leading, indent + (isRoot ? 0 : 14))
                }
                ForEach(node.subfolders) { sub in
                    FolderSection(
                        model: model, node: sub, depth: isRoot ? 0 : depth + 1,
                        isRoot: false, columns: columns)
                }
            }
        }
    }

    private var folderRow: some View {
        Button {
            model.toggleCollapsed(node.id)
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .rotationEffect(.degrees(isCollapsed ? 0 : 90))
                    .foregroundStyle(.secondary)
                Image(systemName: "folder.fill")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Text(node.name)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                Text("\(node.totalCount)")
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(.tertiary)
                Spacer()
            }
            .padding(.leading, indent)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Collapse or expand this folder (\(node.totalCount) images)")
    }
}

/// Thumbnail + index badge (filename available on hover).
struct LibraryCell: View {
    var model: AppModel
    let url: URL
    @State private var image: CGImage?

    private var isSelected: Bool { model.multiSelection.contains(url) }
    private var isCurrent: Bool { model.selection == url }

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            ZStack {
                RoundedRectangle(cornerRadius: 6).fill(.quaternary)
                if let image {
                    Image(decorative: image, scale: 1.0)
                        .resizable()
                        .aspectRatio(contentMode: .fill)
                        .frame(minWidth: 0)
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                } else {
                    ProgressView().controlSize(.small)
                }
            }
            .aspectRatio(3.0 / 2.0, contentMode: .fit)
            .overlay {
                RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(
                        isSelected ? Color.accentColor : .clear,
                        lineWidth: isCurrent ? 3 : 2)
            }
            if let index = model.fileIndex[url] {
                Text("\(index)")
                    .font(.system(size: 9, weight: .semibold).monospacedDigit())
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(.black.opacity(0.55), in: Capsule())
                    .foregroundStyle(.white.opacity(0.9))
                    .padding(4)
            }
        }
        .help(url.lastPathComponent)
        .task(id: url) {
            image = await model.thumbnails.thumbnail(for: url)
        }
        .gesture(
            ExclusiveGesture(
                TapGesture().modifiers(.shift).onEnded { model.selectRange(to: url) },
                ExclusiveGesture(
                    TapGesture().modifiers(.command).onEnded { model.select(url, additive: true) },
                    TapGesture().onEnded { model.select(url, additive: false) }
                )
            )
        )
        .contextMenu { exportMenu }
    }

    @ViewBuilder
    private var exportMenu: some View {
        let urls: [URL] =
            model.multiSelection.contains(url)
            ? model.files.filter { model.multiSelection.contains($0) }
            : [url]
        let title = urls.count == 1 ? "Export Image…" : "Export \(urls.count) Images…"
        Button(title) {
            if !model.multiSelection.contains(url) {
                model.select(url, additive: false)
            }
            model.exportRequest = AppModel.ExportRequest(urls: urls)
        }
        .disabled(model.isExporting)

        Divider()
        // Copy always reads the clicked frame; paste targets the whole
        // multi-selection when the click lands inside it (like export).
        Button("Copy Adjustments") { model.copyAdjustments(from: url) }
        Button(urls.count == 1 ? "Paste Adjustments" : "Paste Adjustments to \(urls.count) Images") {
            model.pasteAdjustments(to: urls)
        }
        .disabled(model.copiedAdjustments == nil)
    }
}
