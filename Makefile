# Command Line Tools ship Testing.framework outside the default search paths
# (and its lib_TestingInterop.dylib in a second directory), so tests need
# explicit framework + rpath flags. `swift build` / `swift run` need nothing.
# Since CLT 27.0 (Swift 6.4, swiftbuild backend) the `@Test`/`@Suite` macro
# plugin (plugins/testing/libTestingMacros.dylib) is also resolved
# unreliably — roughly two runs in three fail with "plugin for module
# 'TestingMacros' not found" on a random test target — so its directory is
# passed explicitly with -plugin-path.
CLT_FW := /Library/Developer/CommandLineTools/Library/Developer/Frameworks
CLT_LIB := /Library/Developer/CommandLineTools/Library/Developer/usr/lib
CLT_TESTING_PLUGINS := /Library/Developer/CommandLineTools/usr/lib/swift/host/plugins/testing
TEST_FLAGS := -Xswiftc -F$(CLT_FW) -Xlinker -F$(CLT_FW) \
	-Xlinker -rpath -Xlinker $(CLT_FW) -Xlinker -rpath -Xlinker $(CLT_LIB) \
	-Xswiftc -plugin-path -Xswiftc $(CLT_TESTING_PLUGINS)

.PHONY: build test run release app install check-state

# The Command Line Tools cannot compile SwiftUI's `@State` (a macro whose
# plugin ships only in Xcode, as of the macOS 27 SDK). Use `@ViewState` from
# Sources/SwiftInvert/ViewState.swift instead. This check keeps a machine
# that happens to have Xcode from reintroducing it.
STATE_PATTERN := '^[^/]*@State([^A-Za-z0-9_]|$$)'
check-state:
	@if grep -rnE $(STATE_PATTERN) Sources/ >/dev/null; then \
	  echo "error: '@State' does not build with the Command Line Tools; use '@ViewState' (see Sources/SwiftInvert/ViewState.swift):" >&2; \
	  grep -rnE $(STATE_PATTERN) Sources/ >&2; \
	  exit 1; \
	fi

build: check-state
	swift build

test: check-state
	swift test $(TEST_FLAGS)

run: check-state
	swift run SwiftInvert

release: check-state
	swift build -c release

# Package the release binary as a real .app so LaunchServices owns the icon
# everywhere (Finder, Dock, ⌘Tab — including the quit animation, where the
# runtime applicationIconImage call can no longer answer for a dying process).
# SwiftPM's generated Bundle.module accessor resolves resource bundles at
# Bundle.main.bundleURL/<name>.bundle — the .app TOP LEVEL — so the two
# resource bundles (Metal shader source; app-icon PNG) are copied there, NOT
# into Contents/Resources. Homebrew dylibs (LibRaw + transitive deps) are
# copied into Contents/Frameworks and re-signed ad-hoc (bundle_dylibs.sh), so
# the .app is self-contained. Individual binaries are ad-hoc signed; the
# BUNDLE is unsigned — fine locally, but strict (distribution) signing
# rejects top-level items besides Contents, so signing later means
# revisiting the resource-bundle placement.
APP_DIR := dist/SwiftInvert.app

app: release
	rm -rf $(APP_DIR)
	mkdir -p $(APP_DIR)/Contents/MacOS $(APP_DIR)/Contents/Resources
	cp Packaging/Info.plist $(APP_DIR)/Contents/
	cp .build/release/SwiftInvert $(APP_DIR)/Contents/MacOS/
	cp -R .build/release/SwiftInvert_SwiftInvert.bundle $(APP_DIR)/
	cp -R .build/release/SwiftInvert_MetalRenderKit.bundle $(APP_DIR)/
	cp Assets/SwiftInvert.icns $(APP_DIR)/Contents/Resources/AppIcon.icns
	scripts/bundle_dylibs.sh $(APP_DIR)
	@echo "Built $(APP_DIR)"

install: app
	rm -rf /Applications/SwiftInvert.app
	cp -R $(APP_DIR) /Applications/
	@echo "Installed /Applications/SwiftInvert.app"
