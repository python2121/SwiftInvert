# Command Line Tools ship Testing.framework outside the default search paths
# (and its lib_TestingInterop.dylib in a second directory), so tests need
# explicit framework + rpath flags. `swift build` / `swift run` need nothing.
CLT_FW := /Library/Developer/CommandLineTools/Library/Developer/Frameworks
CLT_LIB := /Library/Developer/CommandLineTools/Library/Developer/usr/lib
TEST_FLAGS := -Xswiftc -F$(CLT_FW) -Xlinker -F$(CLT_FW) \
	-Xlinker -rpath -Xlinker $(CLT_FW) -Xlinker -rpath -Xlinker $(CLT_LIB)

.PHONY: build test run release app install

build:
	swift build

test:
	swift test $(TEST_FLAGS)

run:
	swift run SwiftInvert

release:
	swift build -c release

# Package the release binary as a real .app so LaunchServices owns the icon
# everywhere (Finder, Dock, ⌘Tab — including the quit animation, where the
# runtime applicationIconImage call can no longer answer for a dying process).
# SwiftPM's generated Bundle.module accessor resolves resource bundles at
# Bundle.main.bundleURL/<name>.bundle — the .app TOP LEVEL — so the two
# resource bundles (Metal shader source; app-icon PNG) are copied there, NOT
# into Contents/Resources. Unsigned local build: fine as-is, but strict code
# signing rejects top-level items besides Contents, so signing later means
# revisiting that placement.
APP_DIR := dist/SwiftInvert.app

app: release
	rm -rf $(APP_DIR)
	mkdir -p $(APP_DIR)/Contents/MacOS $(APP_DIR)/Contents/Resources
	cp Packaging/Info.plist $(APP_DIR)/Contents/
	cp .build/release/SwiftInvert $(APP_DIR)/Contents/MacOS/
	cp -R .build/release/SwiftInvert_SwiftInvert.bundle $(APP_DIR)/
	cp -R .build/release/SwiftInvert_MetalRenderKit.bundle $(APP_DIR)/
	cp Assets/SwiftInvert.icns $(APP_DIR)/Contents/Resources/AppIcon.icns
	@echo "Built $(APP_DIR)"

install: app
	rm -rf /Applications/SwiftInvert.app
	cp -R $(APP_DIR) /Applications/
	@echo "Installed /Applications/SwiftInvert.app"
