# Bundled assets

| Path | Asset | Author | Source | Licence | Added |
|---|---|---|---|---|---|
| `fonts/Inter-Regular.ttf` | Inter (Regular), the UI font used when the system UI font can't be loaded | Rasmus Andersson | https://github.com/rsms/inter (copied unmodified from LightCraft's `assets/fonts/`, itself from the upstream release) | SIL Open Font License 1.1 (`fonts/OFL-Inter.txt`) | 2026-10-16 |

The app icon (`icon_1024.png`, `SwiftInvert.icns`; `_v1` = the earlier design) is the project's own artwork, drawn by `scripts/generate_icon.swift`.
System UI fonts (SF Pro on macOS, the desktop's font on Linux, Segoe UI on Windows) are read
from the user's own system at runtime and are never bundled or redistributed.
