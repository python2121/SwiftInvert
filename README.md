# NegSwift

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
swift build
swift run NegSwift          # the app
swift test                  # parity tests against NegPy-dumped fixtures
.build/debug/negcli         # headless pipeline driver (decode/thumb/render)
```

## Architecture

- `NegativeKit` — pure Swift port of NegPy's conversion kernel: log-density
  analysis, auto-exposure metering, print-curve parameter derivation.
  Ported from `negpy/features/exposure/{normalization,logic,models}.py`.
- `MetalRenderKit` — the per-pixel render chain (normalization → H&D print
  curve → ProPhoto ROMM encode → histogram), ported from NegPy's WGSL shaders.
- `RawDecodeKit` — LibRaw wrapper matching NegPy's rawpy decode parameters
  (linear sensor-native decode, unity WB, EXIF orientation baked after).
- `NegSwift` — the SwiftUI app. `negcli` — headless CLI for verification.

## Parity fixtures

`Tests/Fixtures/` is dumped from the NegPy reference implementation by
`scripts/dump_fixtures.py` (run inside NegPy's environment; see the script
docstring). Swift tests verify each stage boundary against them.
