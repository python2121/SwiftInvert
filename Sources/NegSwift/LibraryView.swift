import SwiftUI

struct LibraryView: View {
    @Bindable var model: AppModel

    private let columns = [GridItem(.adaptive(minimum: 140, maximum: 200), spacing: 8)]

    var body: some View {
        Group {
            if model.folderURL == nil {
                ContentUnavailableView {
                    Label("No folder selected", systemImage: "folder")
                } description: {
                    Text("Choose a folder of camera-scanned negatives.")
                } actions: {
                    Button("Choose Folder…") { model.chooseFolder() }
                }
            } else if model.files.isEmpty {
                ContentUnavailableView(
                    "No RAW files", systemImage: "photo.on.rectangle.angled",
                    description: Text("No supported camera RAW files in this folder."))
            } else {
                ScrollView {
                    LazyVGrid(columns: columns, spacing: 8) {
                        ForEach(model.files, id: \.self) { url in
                            ThumbnailCell(
                                url: url, store: model.thumbnails,
                                isSelected: model.selection == url
                            )
                            .onTapGesture { model.selection = url }
                        }
                    }
                    .padding(8)
                }
            }
        }
        .navigationTitle(model.folderURL?.lastPathComponent ?? "NegSwift")
        .toolbar {
            ToolbarItem {
                Button("Choose Folder…", systemImage: "folder") { model.chooseFolder() }
            }
        }
    }
}

struct ThumbnailCell: View {
    let url: URL
    let store: ThumbnailStore
    let isSelected: Bool
    @State private var image: CGImage?

    var body: some View {
        VStack(spacing: 4) {
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
                    .strokeBorder(isSelected ? Color.accentColor : .clear, lineWidth: 3)
            }
            Text(url.lastPathComponent)
                .font(.caption2)
                .lineLimit(1)
                .foregroundStyle(isSelected ? .primary : .secondary)
        }
        .task(id: url) {
            image = await store.thumbnail(for: url)
        }
    }
}
