# SwiftInvert — Rust + egui + wgpu. Thin wrappers over cargo so the old muscle memory
# (`make test`, `make app`, `make install`) keeps working; everything real lives in Cargo.toml
# and ci.sh.
#
#   make build     debug build of the whole workspace
#   make test      the quality gate: cargo fmt --check, clippy -D warnings, cargo test --workspace
#                  (set NEG_RAW_SAMPLE=/path/file.CR3 to include the real-RAW smoke tests)
#   make run       the app from source, release profile (debug decode is far slower)
#   make release   release binaries: swiftinvert + negcli (target/release/)
#   make app       macOS: package target/release/swiftinvert as dist/SwiftInvert.app
#   make install   copy the bundle to /Applications
#
# CARGO_FLAGS is appended to every cargo build/run, e.g. `make app CARGO_FLAGS=--no-default-features`
# leaves the `--selftest` harness (egui_kittest) out of the product binary. The default keeps it in,
# so the bundled binary can still be verified headlessly:
#   dist/SwiftInvert.app/Contents/MacOS/swiftinvert --selftest out.png <folder>

CARGO_FLAGS ?=
APP_DIR := dist/SwiftInvert.app
BIN := target/release/swiftinvert

.PHONY: build test run release app install clean

build:
	cargo build --workspace $(CARGO_FLAGS)

test:
	./ci.sh

run:
	cargo run --release -p swiftinvert $(CARGO_FLAGS)

release:
	cargo build --release -p swiftinvert -p negcli $(CARGO_FLAGS)

# A plain .app: binary + Info.plist + icon. The Rust app needs no bundled dylibs (lightcraft-raw is
# pure Rust; LibRaw is only linked with `--features libraw`), and it embeds its fonts and icon, so
# there are no resource bundles either. Ad-hoc signed so LaunchServices/Gatekeeper accept it locally.
app:
	@test "$$(uname -s)" = Darwin || { echo "make app packages a macOS bundle; on Linux use packaging/linux/install-desktop.sh" >&2; exit 1; }
	cargo build --release -p swiftinvert $(CARGO_FLAGS)
	rm -rf $(APP_DIR)
	mkdir -p $(APP_DIR)/Contents/MacOS $(APP_DIR)/Contents/Resources
	cp packaging/macos/Info.plist $(APP_DIR)/Contents/
	cp $(BIN) $(APP_DIR)/Contents/MacOS/
	cp assets/SwiftInvert.icns $(APP_DIR)/Contents/Resources/AppIcon.icns
	codesign --force --deep --sign - $(APP_DIR)
	@echo "Built $(APP_DIR)"

install: app
	rm -rf /Applications/SwiftInvert.app
	cp -R $(APP_DIR) /Applications/
	@echo "Installed /Applications/SwiftInvert.app"

clean:
	cargo clean
	rm -rf dist/SwiftInvert.app
