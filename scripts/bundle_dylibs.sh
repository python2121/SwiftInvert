#!/bin/bash
# Bundle the app's Homebrew dylib dependencies (LibRaw and its transitive
# deps: lcms2, libomp, libjpeg-turbo) into Contents/Frameworks so the .app
# runs on machines without Homebrew and survives `brew upgrade`s. Load
# commands are rewritten to @executable_path/@loader_path, and every touched
# Mach-O is re-signed ad-hoc — Apple Silicon refuses to load binaries whose
# signature install_name_tool invalidated.
set -eo pipefail

APP="$1"
EXE="$APP/Contents/MacOS/SwiftInvert"
FW="$APP/Contents/Frameworks"
mkdir -p "$FW"

# copy_dep <path>: copy a Homebrew dylib into Frameworks once and queue the
# copy for its own dependency scan (filenames are versioned, e.g.
# libraw_r.25.dylib, so discovery beats hardcoding a list that rots on the
# next brew bump).
pending=()
copy_dep() {
    local name
    name=$(basename "$1")
    if [ ! -f "$FW/$name" ]; then
        cp "$1" "$FW/$name"
        chmod u+w "$FW/$name"
        install_name_tool -id "@loader_path/$name" "$FW/$name" 2>/dev/null
        pending+=("$FW/$name")
    fi
}

# Patch and sign the executable OUTSIDE the bundle: codesign treats signing a
# bundle's main executable as signing the whole bundle, and the strict
# bundle-format check rejects our top-level SwiftPM resource bundles
# ("unsealed contents present in the bundle root"). A bare file gets a plain
# code signature with no bundle validation; dyld only checks page signatures
# at launch, so the app runs fine.
TMP=$(mktemp -d)
mv "$EXE" "$TMP/SwiftInvert"
for dep in $(otool -L "$TMP/SwiftInvert" | tail -n +2 | awk '{print $1}' | grep '^/opt/homebrew' || true); do
    copy_dep "$dep"
    install_name_tool -change "$dep" "@executable_path/../Frameworks/$(basename "$dep")" \
        "$TMP/SwiftInvert" 2>/dev/null
done

# Transitive deps reference their Frameworks siblings via @loader_path.
while [ ${#pending[@]} -gt 0 ]; do
    bin="${pending[0]}"
    pending=("${pending[@]:1}")
    for dep in $(otool -L "$bin" | tail -n +2 | awk '{print $1}' | grep '^/opt/homebrew' || true); do
        copy_dep "$dep"
        install_name_tool -change "$dep" "@loader_path/$(basename "$dep")" "$bin" 2>/dev/null
    done
done

for lib in "$FW"/*.dylib; do
    codesign -f -s - "$lib" 2>/dev/null
done
codesign -f -s - "$TMP/SwiftInvert" 2>/dev/null
mv "$TMP/SwiftInvert" "$EXE"
rmdir "$TMP"

echo "Bundled $(ls "$FW" | wc -l | tr -d ' ') dylibs into Contents/Frameworks"
