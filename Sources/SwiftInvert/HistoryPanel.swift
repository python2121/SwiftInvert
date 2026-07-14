import SwiftUI

/// Collapsible edit-history panel: undo/redo buttons (⌘Z / ⇧⌘Z) and the full
/// per-image timeline, newest first. Entries ahead of the current state are
/// the redo history (dimmed); any new edit clears them. Click a row to jump.
struct HistoryPanel: View {
    @Bindable var model: AppModel
    /// Open list height, owned and clamped by the parent (the resize handles
    /// around Crop & Rotation both drive it); this panel stays intrinsic.
    var listHeight: Double = 150
    @AppStorage("historyCollapsed") private var collapsed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            if !collapsed {
                historyList
            }
        }
        .padding(.vertical, 4)
    }

    private var header: some View {
        HStack(spacing: 5) {
            Button {
                collapsed.toggle()
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .rotationEffect(.degrees(collapsed ? 0 : 90))
                        .foregroundStyle(.secondary)
                    Text("History").font(.headline)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Collapse or expand the History section")
            Spacer()
            Button {
                model.undo()
            } label: {
                Image(systemName: "arrow.uturn.backward")
            }
            .buttonStyle(.borderless)
            .disabled(!model.canUndo)
            .help("Undo (⌘Z)")
            Button {
                model.redo()
            } label: {
                Image(systemName: "arrow.uturn.forward")
            }
            .buttonStyle(.borderless)
            .disabled(!model.canRedo)
            .help("Redo (⇧⌘Z)")
        }
        .padding(.horizontal, 12)
    }

    private var historyList: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 1) {
                ForEach(Array(model.historyEntries.enumerated()).reversed(), id: \.element.id) { index, entry in
                    row(index: index, entry: entry)
                }
            }
            .padding(.horizontal, 8)
        }
        .frame(height: listHeight)
    }

    private func row(index: Int, entry: AppModel.HistoryEntry) -> some View {
        let isCurrent = index == model.historyIndex
        let isRedo = index > model.historyIndex
        return Button {
            model.jumpToHistory(index)
        } label: {
            HStack(spacing: 6) {
                Text("\(index)")
                    .font(.system(size: 9).monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .frame(width: 16, alignment: .trailing)
                Text(entry.label)
                    .font(.caption)
                    .lineLimit(1)
                    .foregroundStyle(isRedo ? .tertiary : (isCurrent ? .primary : .secondary))
                Spacer()
            }
            .padding(.vertical, 3)
            .padding(.horizontal, 4)
            .background(
                isCurrent ? Color.accentColor.opacity(0.22) : .clear,
                in: RoundedRectangle(cornerRadius: 4))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(isRedo ? "\(entry.label) (redo)" : entry.label)
    }
}
