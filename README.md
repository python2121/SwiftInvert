# SwiftInvert

A minimal native macOS rewrite of [NegPy](../NegPy)'s film-negative conversion:
a library of camera-scanned negatives, C-41 conversion with NegPy-quality
auto-exposure metering, basic exposure controls (density, grade, white balance,
cast removal), and a histogram. Swift + SwiftUI + Metal, no Python at runtime.

## Requirements

- macOS 14+, Swift 6 toolchain (Command Line Tools are enough — no Xcode needed;
  Metal shaders compile at runtime)
- `brew install libraw` (dynamically linked, LGPL)

## Build & run

```bash
swift run -c release SwiftInvert   # run from source (debug decode is ~10x slower)
make app                    # package dist/SwiftInvert.app
make install                # ...and copy it to /Applications
make test                   # parity tests against NegPy-dumped fixtures (not bare `swift test`)
.build/release/negcli       # headless pipeline driver (decode/thumb/render)
```

## Architecture

- `NegativeKit` — pure Swift port of NegPy's conversion kernel: log-density
  analysis, auto-exposure metering, print-curve parameter derivation.
  Ported from `negpy/features/exposure/{normalization,logic,models}.py`.
- `MetalRenderKit` — the per-pixel render chain (normalization → H&D print
  curve → ProPhoto ROMM encode → histogram), ported from NegPy's WGSL shaders.
- `RawDecodeKit` — LibRaw wrapper matching NegPy's rawpy decode parameters
  (linear sensor-native decode, unity WB, EXIF orientation baked after).
- `SwiftInvert` — the SwiftUI app. `negcli` — headless CLI for verification.

## Parity fixtures

`Tests/Fixtures/` is dumped from the NegPy reference implementation by
`scripts/dump_fixtures.py` (run inside NegPy's environment; see the script
docstring). Swift tests verify each stage boundary against them.

## License

SwiftInvert is licensed under the [GNU General Public License v3.0](LICENSE).
LibRaw is used under its LGPL-2.1 option via dynamic linking (LGPL-2.1 §3
permits GPL conversion, making it GPL-3.0-compatible).
