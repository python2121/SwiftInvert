#!/usr/bin/env bash
# Install (or remove) the SwiftInvert desktop entry + icon for the current user, so Linux
# desktops show the app's own icon. Wayland compositors (KDE Plasma, GNOME) ignore a window's
# icon and take it from the .desktop file whose name matches the app id — `swiftinvert`, which
# the app sets (`with_app_id("swiftinvert")`). X11 also reads the icon the window sets itself.
#
#   packaging/linux/install-desktop.sh [path/to/swiftinvert]   # default: target/release/swiftinvert
#   packaging/linux/install-desktop.sh --uninstall
#
# User scope only (~/.local/share); no root, nothing outside these two files.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons"
desktop="$apps/swiftinvert.desktop"
icon="$icons/swiftinvert.png"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$desktop" "$icon"
    command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
    echo "removed $desktop and $icon"
    exit 0
fi

bin="${1:-$repo/target/release/swiftinvert}"
bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
[[ -x "$bin" ]] || { echo "no executable at $bin — build it first: cargo build --release -p swiftinvert" >&2; exit 1; }

mkdir -p "$apps" "$icons"
# the app icon (1024 px with macOS's standard margin)
install -m 644 "$repo/assets/icon_1024.png" "$icon"
cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=SwiftInvert
GenericName=Film Negative Converter
Comment=Convert scanned colour negatives from camera RAW files
Exec="$bin" %f
Icon=$icon
Terminal=false
Categories=Graphics;Photography;
StartupWMClass=swiftinvert
DESKTOP
chmod 644 "$desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
echo "installed $desktop (Exec=$bin) and $icon"
