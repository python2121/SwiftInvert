# CLAUDE.md

Guidance for Claude Code when working in this repository.

> **Keep this file current.** When a change alters something documented here —
> pipeline stage order, the uniform contract, an analysis default, the sidecar
> format, or the build/test commands — update the relevant section in the same
> change. After adding an adjustment control, re-check the "Adding a new
> adjustment control" checklist and the uniform layout table.

SwiftInvert is a native macOS film-negative conversion app (SwiftUI + Metal),
a deliberate ~10% rewrite of the Python app **NegPy**
(`~/Documents/code/NegPy`), which remains the numerical reference
implementation. Scope: library grid, C-41 conversion with NegPy-quality auto
metering, exposure/color controls, histogram, export. Camera RAW only; no
B&W/E6, papers, crosstalk, retouch, or toning.

## Commands

```bash
swift build                 # debug build
swift run -c release SwiftInvert   # the app (release: debug decode is ~10x slower)
make test                   # Swift Testing suite — MUST use make, not bare `swift test`
swift build -c release && .build/release/negcli   # headless CLI (decode/thumb/render/bench/meter)
make app                    # package self-contained dist/SwiftInvert.app (bundles Homebrew dylibs)
make install                # copy the bundle to /Applications

# Regenerate parity fixtures (after deliberate NegPy kernel/constant changes):
cd ~/Documents/code/NegPy && uv run python ~/Documents/code/SwiftInvert/scripts/dump_fixtures.py

# Linux (SteamOS, Qt-frontend port in progress) — inside the `swiftdev`
# distrobox (Ubuntu 24.04; Swift via swiftly; apt libraw-dev/liblcms2-dev/
# libvulkan-dev/glslang-tools/mesa-vulkan-drivers):
distrobox enter swiftdev -- bash -lc 'cd ~/Documents/code/SwiftInvert && swift build && swift test'
# Bare `swift test` DOES work on Linux (Swift Testing ships in the toolchain).
# negcli builds there too (decode/thumb/render/bench/meter, Vulkan render).

# Regenerate the checked-in SPIR-V after editing NegPipeline.comp:
./scripts/compile_vulkan_shaders.sh   # (in the box; needs glslangValidator)

# Qt shell (in the box; apt qt6-base-dev qt6-base-dev-tools cmake ninja-build):
swift build -c release --product SwiftInvertCore
cmake -S qt -B qt/build -G Ninja && cmake --build qt/build
qt/build/swiftinvert-qt [folder]         # GUI (distrobox passes the display)
QT_QPA_PLATFORM=offscreen qt/build/swiftinvert-qt --selftest out.png <folder>
```

**Linux port status:** `Package.swift` computes its target list — the
portable core (CLibRaw, NegativeKit, RawDecodeKit, NegativeKitTests),
negcli, and the Linux GPU stack (CVulkan, VulkanRenderKit + its tests)
build on Linux; MetalRenderKit, SwiftInvert and their test targets are
gated behind `#if os(macOS)` (the manifest runs on the build host, which
is exactly the right question for Metal/SwiftUI; per-branch declarations
because SwiftPM validates conditional dependency names even when false).
Apple-only APIs in the core sit behind `#if canImport(Accelerate)` with
scalar fallbacks (`Prefilter.logImage`, `RawDecoder`'s u16→float) and
`#if canImport(simd)` with stdlib-backed shims
(`NegativeKit/SimdShims.swift` — the full parity suite passes on Linux
against the same fixtures, so the fallbacks are pinned).

**VulkanRenderKit** is the Linux mirror of MetalRenderKit's chain:
- `Shaders/NegPipeline.comp` — GLSL port of NegPipeline.metal, all six
  kernels (`normalize`/`print_curve`/`color_pop`/`histogram`/`encode_f`/
  `encode_u8`) in one file selected by `-DKERNEL_*`; the checked-in `.spv`
  binaries are the build inputs (`scripts/compile_vulkan_shaders.sh`
  regenerates — rerun + commit them with ANY `.comp` edit).
- SSBOs of interleaved RGB floats (RGBImage's own layout — upload/readback
  are memcpys), 1-D dispatch, std140 uniform blocks that are byte-identical
  to the shared `ShaderTypes.swift` (a SYMLINK into MetalRenderKit — one
  source of truth for uniform packing; `VulkanLayoutTests` pins offsets).
- **Memory discipline (learned at 3.9 s/frame):** readback targets
  (encoded float, display u8, histogram) MUST be `hostReadback: true` —
  HOST_CACHED system RAM. Everything else sits in DEVICE_LOCAL|HOST_VISIBLE
  (BAR), which the CPU must never read: BAR is write-combined and uncached
  reads are ~1000× slower. `DeviceBuffer` holds the context strongly so
  the VkDevice outlives every buffer (teardown order crashed otherwise).
- `VulkanParityTests` verify GPU vs the CPU `ReferenceCurve` chain at the
  Mac suite's gates (mean<0.01, max<0.04) across every control group —
  chaining the Vulkan renderer to the NegPy fixtures through the CPU
  reference. `NEGCLI_VK_CPU=1` forces llvmpipe where no GPU is visible.
- Measured on the Steam Machine (RADV NAVI33) at 1536px: **1.7 ms/frame
  (~590 fps) renderDisplay incl. readback**; 24 MP export render 0.39 s.
  The CPU `ReferenceCurve` chain (~509 ms/frame) remains the no-GPU
  fallback in negcli.

negcli's Linux TIFF output is ImageIO-free (`negcli/TIFF16.swift`,
untagged baseline TIFF — bytes are Adobe RGB-encoded); littleCMS-based
display/export color management is still to come, converging with NegPy's
own stack.

**The Qt shell** (`qt/`, Linux only) is deliberately thin: all pipeline
semantics live in Swift behind **CoreBridge**
(`Sources/CoreBridge/CoreBridge.swift` → `libSwiftInvertCore.so`, C ABI
mirrored in `qt/swiftinvert_core.h` — keep the two in sync). Bridge
contract: sessions are int64 handles (`si_open` = preview decode + analyze
+ GPU upload; ~1 s, off the UI thread), settings travel as SIDECAR-FORMAT
JSON (missing keys = defaults, so the frontend never re-implements
fields — `si_default_settings` seeds the sliders), every returned buffer
is malloc'd/`si_free`'d, `si_render` returns rgba8 via Vulkan
renderDisplay (~2–5 ms), `si_thumbnail` returns the embedded camera JPEG
(Qt decodes it). `qt/main.cpp` (Widgets + CMake) is folder browsing, a
fit-to-window canvas, controls editing the settings JSON with latest-wins
async renders, and a `--selftest <png>` mode that renders + screenshots
offscreen (`QT_QPA_PLATFORM=offscreen`) — the headless way to verify UI
changes.

Shell feature state (Phase A, 2026-08-08):
- **Controls mirror `ControlsSidebar` exactly** — same sections, ranges,
  defaults and direction conventions (Brightness = 2 − density, right =
  brighter), per-control reset-⨯ + double-click reset, Separation Damping
  disabled at printSaturation 1, mixer/grading band pickers. When the Mac
  sidebar gains a control, add it to `makeControlsPanel` with the same spec.
- **Histogram** from the render's 4×256 bins (`si_render`'s out-param),
  drawn with HistogramView's shape-only rule (peak-normalize, fixed
  log1p compression 100000).
- **Sidecars** round-trip with the Mac: `si_sidecar_load/save` implement
  SidecarStore's exact naming + `{version, settings}` payload (bridge
  decode→encode canonicalizes); Qt debounces saves 1 s, flushes on
  file-switch/close, and merges loaded keys over defaults.
- **sRGB display transform**: `si_render(srgb_display=1)` →
  `renderDisplay(srgbDisplay:)` → flags bit of the (now 8-byte) push
  constants; `encode_u8` decodes the Adobe TRC, gamut-matrixes to sRGB
  and applies the piecewise OETF after the levels remap. Float/export
  paths and the Mac semantics untouched; `srgbDisplayFlagMatchesCPUTransform`
  pins the transform. Proper ICC (lcms2) lands with export.
- **View Original** hold renders stock settings ("{}").
- `si_session_info` feeds the negative-character row (density range,
  label, cast confidence).

Phase B (2026-08-08) — geometry + canvas interactions:
- The bridge `Session` is a miniature ImageSession cache tower with the
  SAME semantics: orientation baked into pixels; analysis on the
  ORIENTATION-ONLY image (straighten never re-meters) scoped by
  crop/analysis rects; render source = oriented(+fineRotation, inscribed)
  THEN cropped — so cropRect is normalized on the fine-rotated frame.
  All tiers keyed; slider drags rebuild nothing.
- Qt tools exploit that JSON contract with zero new bridge surface: the
  CROP tool renders with cropRect stripped (the drawn box IS the stored
  rect, straighten slider ±45° edits fineRotation live; Apply commits,
  Cancel/Escape restores the entry snapshot); the ANALYSIS tool renders
  orientation-only (cropRect+fineRotation stripped) so its rect maps 1:1
  to the metering space (analysisRectFineRotation written as 0).
- Zoom/pan on the canvas (wheel to cursor, drag pan, double-click
  fit↔100%); rotate L/R (Ctrl+[/]) and flip (Ctrl+Shift+H) via toolbar.
- Per-file session history (undo Ctrl+Z / redo / click-to-jump list at
  the sidebar bottom): slider drags commit one entry on release,
  programmatic sets and toggles commit immediately, entries hold full
  settings JSON, redo tail truncates, capped at 100.
- `--selftest` now also captures `<base>_crop.png` in crop mode.

Phase C (2026-08-08) — export:
- Bridge `si_export_render` (RGBA8 for the frontend's JPEG encode) and
  `si_export_tiff` (untagged 16-bit baseline TIFF written by the core):
  full-resolution best-demosaic decode + the same orientation → fine
  rotation → crop chain, but analysis from the PREVIEW-SIZED proxy — the
  what-you-see invariant, matching the Mac's exportRender-shares-prepare.
  Options JSON `{"colorspace":"srgb"|"adobe","maxLongEdge":N}`; the sRGB
  leg runs in-kernel (`encode_f` honors the same flags bit as the display
  kernel; `render(srgbEncode:)`), resize on the encoded output via
  `downsampled` (Mac order). `negcli/TIFF16.swift` is symlinked into
  CoreBridge (one writer, two targets).
- Qt: Export… (Ctrl+E) dialog mirroring ExportSheet — JPEG (quality,
  default 92) / TIFF-16, sRGB (default) / Adobe, resize long edge
  (default on, 3000), next-to-source or chosen folder, sticky via
  QSettings; batch over the library multi-selection, each frame using its
  own sidecar (current frame flushed first); Mac naming (same basename,
  .jpg/.tiff, overwrite).
- Still missing (Phase D+): test strip, levels drag on the histogram,
  densitometer/zones, HQ display tier, ICC-tagged output (lcms2).

**Toolchain constraints (this machine has Command Line Tools, no Xcode):**
- No XCTest and Testing.framework lives outside default search paths → tests
  use Swift Testing with the framework/rpath flags encoded in the `Makefile`.
  Bare `swift test` fails with "no such module 'Testing'".
- No build-time `metal` compiler → shaders are compiled **at runtime** from
  `Sources/MetalRenderKit/Shaders/NegPipeline.metal` (a bundled `.copy`
  resource) via `MTLDevice.makeLibrary(source:)`.
- LibRaw comes from Homebrew (`brew install libraw`), linked dynamically via
  the `CLibRaw` systemLibrary target (`pkgConfig: "libraw_r"`). LGPL-2.1.
- If the repo directory is ever moved/renamed: `rm -rf .build` (the module
  cache embeds absolute paths).

## Module graph

```
SwiftInvert (app)  ──►  MetalRenderKit ──► NegativeKit
       │                     │
       └──►  RawDecodeKit ───┴──► NegativeKit
                  │
                  └──► CLibRaw (Homebrew libraw_r)
negcli ──► RawDecodeKit + NegativeKit + MetalRenderKit
```

- **NegativeKit** — pure Swift math, no UI/Metal/IO. The port of NegPy's
  conversion kernel. Only Foundation/simd/Accelerate.
- **MetalRenderKit** — the GPU render chain + `ColorIO` (CGImage/color-space
  conversion, used by export).
- **RawDecodeKit** — LibRaw wrapper + embedded-thumbnail extraction.
- **SwiftInvert** — SwiftUI app (`AppModel`, `ImageSession`, views).
- **negcli** — headless driver; `bench` measures slider-latency and analysis;
  `meter` prints the darkroom read-outs (negative character + spot-densitometer
  probes) for a real RAW, which is how the read-out math gets exercised on
  actual scans (the fixtures can't cover a hover).

## The image pipeline

End-to-end: **decode → orient → analyze (CPU) → derive params (CPU, µs) →
render (GPU) → display/export**. Slider changes re-run ONLY derive+render
(~5 ms at preview size). Analysis re-runs only when its inputs change.

### 1. RAW decode (`RawDecodeKit/RawDecoder.swift`)

LibRaw parameters mirror NegPy's rawpy decode exactly (verified byte-identical
against rawpy on real files — a rawpy comparison must now pass
`adjust_maximum_thr=0` explicitly): `gamma=(1,1)`, `no_auto_bright`,
`adjust_maximum_thr=0` (pin the u16 scale to the camera white level; the 0.75
default rescales to the frame's own brightest pixel once it nears clipping —
NegPy 2a6cb22), `output_bps=16`,
`output_color=RAW` (sensor-native, **no camera color matrix**), unity WB
(`user_mul=(1,1,1,1)`, `use_camera_wb=0`), `user_flip=0`. Output is scene-linear
`RGBImage` (interleaved RGB float32 in [0,1], `u16/65535` via vDSP).

- **Preview path**: `half_size=1` + linear demosaic (`user_qual=0`) — except
  X-Trans sensors (`idata.filters == 9`), where half_size aliases the 6×6 CFA;
  they decode full and downsample. Preview is capped at 1536 px long edge —
unless HQ preview is engaged, when the display render runs on the cached
full-resolution decode (`ImageSession.hqSourceTexture`; analysis stays on
the proxy, matching export).
  **`AppModel.HQMode`** (canvas-bar badge cycles off → auto → on, ⇧⌘P, View
  menu picker; session-only — a persisted `.on` would make every launch pay
  full-res costs): **`.auto` is the default** and stays on the proxy until
  the canvas is magnified to `hqAutoZoomThreshold` (**2×**). The proxy stays
  on screen through any swap, so it reads as sharpening rather than a reload,
  and dropping back below is instant off the warm tower.
  **Three source tiers** (`ImageSession.RenderTier`, resolved by
  `AppModel.renderTier(mode:active:)`; analysis ALWAYS runs on the 1536px
  proxy, so switching tiers can never move the conversion):
  `.proxy` 1536px · `.mediumIfFree` · `.full`.
  The **medium tier** is the buffer the PREVIEW decode already produced and
  used to discard — 3012×2010 on a 24MP Bayer body, 4× the proxy's pixels
  for 0 ms and ~73 MB. `prepare()` therefore decodes at native preview size
  and downsamples to 1536 itself rather than asking the decoder to cap
  (`basePreview` is still ONE downsample from that buffer, so the analysis
  input is byte-identical). Retained only under
  `mediumTierPixelBudget` (20 MP): an X-Trans preview decodes full-size, and
  those bodies get no medium tier — `.mediumIfFree` falls through to `.full`,
  i.e. today's behaviour. **`.auto` uses the medium tier and so never
  triggers a decode** (instant, but the half-size linear demosaic — more
  resolution, not export colour); **`.on` uses `.full`** and is the
  export-truth answer, paying the ~500–700 ms decode. `DisplayTier` holds the
  base/oriented/texture bookkeeping once and is instantiated per tier. `zoom == 1` is
  fit-to-window, so a "proxy is out of real pixels" threshold would fire at
  rest on any Retina display and make auto indistinguishable from on; 2×
  keeps the trigger a property of the gesture, not the window size.
  Resolution is the pure `HQMode.resolve` (tested without a pipeline):
  suppressed with no selection and while the test strip or zone placement
  owns the canvas (both are proxy-renders, and an HQ badge over them would
  lie — this replaces the old boolean's explicit force-off); auto (not on)
  is also suppressed in Crop & Straighten, which pins the canvas to fit
  regardless of zoom. Baseline hold is deliberately NOT suppressed: a
  compare must show both states at the same resolution. `render(retainHQ:)`
  keeps the tier alive across proxy renders in auto/on so zooming out and
  back doesn't re-pay the ~700 ms decode; it is freed on `.off`, and
  `releaseHQ()` still strips it from every session the LRU isn't showing.
  Zoom itself stays DetailView `@State` and is reported to
  `AppModel.canvasZoom`, whose didSet acts only on a threshold CROSSING.
  **Tier-swap stability** — the two tiers must put the same content on the
  same pixel, or the picture nudges when HQ arrives (and `scaleEffect`
  multiplies any error by the zoom, i.e. worst exactly when the swap fires).
  `pixelROI` truncates and `fineRotated` floors on each tier's own grid, so
  a 1536px and a 6024px render of one setting genuinely cover different
  windows, and the proxy was additionally WARPED. Three layers:
  1. **Lay out from `RenderOutput.displayAspect`** (`CropGeometry
     .displayAspect`: continuous frame → inscribed → crop, never a pixel
     count) — bitmap ratios differ up to 0.2% between tiers.
  2. **Place each bitmap at the window it really covers**
     (`RenderOutput.contentWindow` → DetailView's `contentSize`/
     `contentOffset`), instead of stretching both to fill. Both tiers then
     resolve to the same affine frame→screen map, so the crop ROI landing on
     a different pixel in each is corrected EXACTLY rather than merely
     shrunk. **`contentWindow` must measure a tier's bitmap against ITS OWN
     frame** (`orientedFrameSize(of:)`) — using the proxy's frame for the HQ
     bitmap yields a window ~3.9× the ideal rect. The probe normalizes by
     the image's frame, not `window`. (An earlier layer that re-derived the
     HQ crop window from the proxy's was removed once this landed: it only
     shrank the error, and this removes it.)
  3. **`RGBImage.downsampled` is a real area filter.** Integer box
     boundaries (`Int(ox·s)`) give boxes of varying width whose centres
     drift from the ideal grid — 0.97 source px at the preview's 1.96 ratio,
     sliding smoothly across the frame. That is a low-frequency geometric
     WARP of the proxy, so no rigid correction could fix it; it read as
     local misalignment (~1.5 pt at 4× zoom) against the true full-res tier.
     Now separable fractional-weight taps (`areaTaps`), which is cv2's
     INTER_AREA — matching NegPy's preview resize
     (`services/rendering/image_processor.py`) in kernel, not just intent.
     This MOVED the default look slightly (it feeds analysis): probes shift
     ~0.05 D, one test frame crossed a zone boundary. Separable keeps it
     free (prepare ~17 ms, unchanged; the naive 2-D form cost 28.8 ms).
  `DisplayAspectTests` pins tier-equality of the final screen position and
  `ImagePipelineSeamTests` pins the filter's uniform sample phase; both
  deliberately keep cases asserting the OLD behaviours really did drift, so
  the equality tests can't go vacuous.
- **Full path** (export): LibRaw default (best) demosaic, full resolution.
- EXIF orientation baked in Swift afterwards (`applyingFlip`, dcraw codes:
  3=180°, 5=90°CCW, 6=90°CW). C struct's flexible array member must be read
  via byte offset (`MemoryLayout.offset(of: \.data)`) — `withUnsafePointer(to:
  pointee.data)` copies to a misaligned temporary and traps.
- **User orientation** (`settings.rotation` 0/90/180/270 CW +
  `flipHorizontal`) is baked into pixels right after decode
  (`RGBImage.oriented`), so analysis, display-space rects, rendering and
  export all share one coordinate space with zero mapping code.

### 2. Analysis (`NegativeKit`, CPU, on the ≤1536px preview)

**Fine-rotation invariant:** analysis runs on the orientation-only image
(90° steps + flip — `ImageSession.meterPreview`), never the fine-rotated
one: the inscribed auto-crop changes with the straighten angle, and
re-metering it would drift the conversion as the user rotates (it also
made the 0° straighten base's look differ from the committed re-bake).
Rendering/export still bake the angle; `exportRender` shares `prepare()`,
so preview and export agree.

Two stages, both offset-independent since the 2125a34 port
(`ExposureKernel.prepare` ≈17 ms once per image/crop; `finalize` ≈7 ms,
cached with it — white/black-point drags re-run NO analysis at all; the
offsets fold into `finalBounds` at derive time only). Both were ~5.5× slower
until `Stats.sortedAscending` moved off `vDSP_vsort` to `RadixSort`
(prepare 120 → 21 ms, finalize 45 → 7 ms): sorting was ~70% of analysis and
vDSP was the slowest option measured (21.6 ms/252k Floats vs 15.8 for
`Array.sorted()` and 1.9 for radix). **Every order statistic funnels through
that one function**, so keep new percentile/median code going through
`Stats` rather than calling `.sort()` directly:

**`prepare(linearImage:cropRect:analysisRect:analysisBuffer:)` →
`Prepared`** (offset-independent, cached per rect state):
1. **Region priority** (NegPy `resolve_analysis_region`): a freehand
   `analysisRect` wins and disables the buffer inset; else the output
   `cropRect` scopes metering (borders outside the crop can't skew the
   inversion) with the buffer applied inside; else the centered buffer alone.
   Default buffer **0.10** = middle 80% of the frame (NegPy ships 0.05 —
   deliberate divergence). Degenerate rects (<2 px) are ignored.
2. **Prefilter** (`Prefilter.prefilterLogGrid`): the buffer crop, then
   `log10(clip(x, 1e-6, 1.0))` via vForce (buffer holds negative density −D
   since values ≤1), then a **block-median grid**: block side
   `b = ceil(maxDim/1024)`; per-cell, per-channel median kills
   dust/speculars and makes stats resolution-invariant. The preview path
   always hits `b=2`, which has a closed-form median-of-4 fast path.
   NegPy logs BEFORE cropping; we crop first because `log10` is elementwise
   (identical survivors) and the 0.10 buffer discards 36% of the frame.
3. **Bounds** (`BoundsAnalysis.analyze`, NegPy `analyze_log_exposure_bounds`):
   two independent percentile axes on the grid, one sort per channel
   (vDSP): the **luma axis** (base clip 0.01%) sets the mean center+span
   (dynamic range); the **colour axis** (base clip 1.0%) sets per-channel
   cast offsets relative to the median channel. Since the 127bcd7 port
   (NegPy 0.43) the colour axis's dense end (floors = print whites) prefers
   **same-pixel refs** (`samePixelColorFloorRefs`, float64 end-to-end like
   upstream): one shared chroma-gated pixel set — the luma-extreme band
   `[clip, clip+4]` percentile, lowest-RMS-chroma 30%, chroma base-anchored
   against the thin-end refs with a two-pass provisional-gamma refinement —
   so coloured highlight content can't masquerade as film cast; falls back
   to the percentile pass when the band has no trustworthy neutrals
   (< 64 px or median chroma over the caps). Thin-end ceils stay
   percentile-based (film density is bounded below by base). Recombined:
   `floor[ch] = mean(luma floors) + (colour floor[ch] − median(colour floors))`
   (same for ceils) → `LogNegativeBounds` (floors < ceils).
4. **Meters** (all on the same grid):
   - `anchor`: P50 of grid luma normalized by the **base** bounds, pulled 20%
     from `assumedAnchor` 0.46, clamped ±0.12 — the auto-exposure key.
   - `texturalRange`: |P90 − P10| of raw log luma — drives Auto Contrast.
   - `shadowRefs`: P98 per channel — the cast-removal fallback tie.

**`finalize(prepared)` → `ExposureAnalysis`**: the **neutral axis**
(NegPy `measure_neutral_axis_from_log`) measured against the PRE-trim base
bounds — NegPy 2125a34: the film's cast is a source property, so creative
WP/BP trims must not perturb it (their GPU always measured pre-trim; the
CPU side we'd ported was the bug). Two-pass since the 127bcd7 port
(NegPy 0.43): three normalized-luma bands (highlight 0.10–0.30,
mid 0.40–0.60, shadow 0.72–0.92); per band, keep the lowest-**RMS**-chroma
30% (hue-uniform pairwise RMS, not max−min; ≥64 px). Pass 1 selects under a
loose cap (0.55 — admits strong-but-correctable casts, rejects saturated
content); the affine R/B→G correction implied by its mid+shadow refs
re-ranks chroma; pass 2 selects true neutrals under the strict cap (0.29)
and takes per-channel **raw-log medians**. `confidence = tightness ×
sampleSize × agreement` — corrected mid/shadow chroma vs the cap,
`n/(n+256)` on the mid set, and mid↔shadow deviation agreement (0.10 dead
zone, 0.20 roll-off) — gates Auto cast removal. Returns nil mid or shadow
(either pass) → no neutral axis (shadow-ref fallback used downstream).

`ImageSession` caches: decode (per image) → oriented preview (per
orientation) → `Prepared` + `ExposureAnalysis` (per rects) → GPU source
textures (per crop). The "Analyzing…" indicator shows only when `prepare`
will run.

`AppModel.sessionLRU` retains the **2** most recent sessions (active + the one
navigated from), so arrowing back to a frame doesn't rebuild the whole tower.
Retention alone is safe *because* every tier is keyed — a returning session
re-validates against the current settings and cannot serve stale pixels. Only
the on-screen frame may hold the HQ tier (`releaseHQ()` strips it from the
others); the proxy caches are tens of MB, the HQ decode is hundreds.

### 3. Parameter derivation (`ExposureKernel.deriveRenderParams`, µs per tick)

Turns `(ExposureSettings, ExposureAnalysis)` into `RenderParams` — the full
GPU uniform payload. Order matters:

1. `finalBounds = baseBounds + (wp, bp)` offsets (floors+wp, ceils+bp).
2. `dMin = paperDmin ? 0.06 : 0`; `anchor = autoExposure ? metered : nil`.
3. `strength = confidence × slider` — confidence scaling is always on
   (NegPy 0.36 removed the auto toggle; slider 0–2, >1 overcorrects past the
   neutral axis; kernel clamps bound any value).
4. **`perChannelCurveParams`** (`CurveLogic`, the C-41 gray-balance heart) —
   NegPy-exact. Green is the reference (its pivot rides the anchor). Modes:
   - *Neutral-axis* (default): R/B fitted to green's axis — quadratic through
     3 green-matched points when a highlight ref exists (divided-differences
     solve; curvature clamped to ±0.45·slope), else a 2-point line; midtone
     deviations clamped ±0.2.
   - *Shadow-ref fallback*: one-point slope tie at P98 (clamp ±0.1).
   - *Base*: single shared linear curve.
   Inputs: `baseSlope = gradeToSlope(grade, effectiveGradeRange(...))` —
   grade is ISO R 50–180 (115 ≈ grade 2), `k = 2.9·range/(R/100)` clamped
   [2,10]; Auto Contrast damps the floor-ceil/textural ratio toward 2.0.
   `lumRange` comes from the **pre-offset** base bounds (NegPy quirk).
   `computePivot` solves so the reference tone prints at density 0.74.
5. **Overall contrast** folds *exactly* into the core: `v→v+k(v−v*)` ⇒
   slopes,curvatures ×(1+k), pivot += k·v*/s′ (anchor invariant). k =
   slider×0.5, slider −1…+2.
6. **`cmyOffsets`** (pre-curve, normalized space): WB filtration
   (`slider×0.2 / channelRange`) with **Temp** folded along the Planckian
   direction (yellow + magenta×0.0029/0.0057) and **Tint** on magenta, plus
   **Exposure** stops (`stops × −log10(2)/channelRange` — the dodge/burn EV
   domain; + = brighter).
7. Grade-coupled toe/shoulder knees; band CMY (`colorShadows/Mids/Highs`
   ×0.2 into density units); tone controls passthrough (shadowContrast's
   negative side remapped so slider −3 lands on the monotone floor −0.8,
   also hard-clamped in the kernels); `preSaturation`, `vibrance`,
   `saturation` passthrough.

### 4. GPU render (`MetalRenderKit`, rgba32float, 8×8 threadgroups)

One command buffer, passes in order (`RenderPipeline.render` /
`renderDisplay`):

1. **`normalizeLog`** — `log10(max(c,1e-6))` (no upper clamp, matches NegPy
   GPU) → per-channel stretch `(log − floor)/(ceil − floor)` (offsets already
   folded into `finalBounds`; uniform wp/bp fields are legacy-zero).
2. **`printCurve`** — the asymmetric H&D print curve, per pixel:
   a. *Pre-saturation*: `c → mean + k(c − mean)` (density-deviation gain;
      default slider 1.15, kernel identity at 1.0).
   b. Per channel: `val += cmyOffsets` → quadratic core
      `v = slope(val − pivot) + curv·val²` → midtone paper-S
      `v += 0.15·0.6·tanh((v − v*)/0.6)` → **tone controls** (masks
      `wS = σ(3.5(v−1.40))`, `wH = σ(3.5(0.30−v))` on the incoming v,
      parallel form: shadows/highlights lifts + anchor-pivoted contrasts) →
      **3-band color** (same masks, `wM = max(1−wS−wH,0)`; NegPy's 2-band
      regional CMY generalized) → shoulder softplus toward `d_min_eff`
      (paper white) → toe softplus toward `d_max_eff` (paper black) →
      **Print Saturation** (NegPy 0.45 port, on density above paper base,
      identity at default 1.0): uniform k around the per-pixel achromatic
      mean — the global reduction of NegPy's saturation matrix (their
      surviving density-space control, renamed dye_separation in 3fb5ca8;
      per-channel trims not ported; the per-pixel spread-masked Dye
      Separation was ported and retired 2026-07-29 when upstream deleted
      it as redundant — its sidecar key decodes ignored).
   c. `t = 10^−D`; **True Black** (BPC, optional): `t → (t−b)/(1−b)` with
      `b = 10^−dMax` referenced to the *physical* d_max so toe lifts survive
      (negative toe raises the clip point); clamp [0,1] → **linear
      reflectance** out.
3. **`colorPop`** (dispatched when a color-pop control is off-default —
   vibrance/saturation/band controls, hueTrim — or skinProtection > 0, which is
   the DEFAULT (0.5): in practice the pass runs every frame, accepted with
   the port — though the ~7.2 ms/frame recorded then no longer reproduces;
   bench measures 4.3–4.6 ms/frame with the pass confirmed dispatching)
   — CIELAB
   (Adobe RGB (1998) primaries, D65 since the b3490eb port; matrices
   duplicated in MSL and
   `LabColor.swift` — keep in sync): **Hue Trim** first
   (`LabColor.applyHueTrim`, NegPy 7a07f5c port — `hueTrim`, degrees ±30,
   0 = off, radians in `RenderParams`): a rotation of a*/b* about the
   neutral axis. It corrects the CAPTURE, not the negative — a narrowband
   or odd-phosphor scanning light ROTATES hues instead of casting them, and
   white balance cannot fix that because no grey is wrong (upstream measured
   |ΔH| flat at 16–21° from chroma 4 to 60+; a cast would shrink with
   chroma, a channel mix would grow). A rotation fixes the origin, so
   neutrals cannot move and it can never fight the cast removal. Runs FIRST
   so every control below acts on corrected hues, matching upstream where it
   rides the exposure pass ahead of their Lab stage. DIVERGENCE: upstream
   makes it sticky across frames; ours is an ordinary sidecar setting —
   bake it into a `SettingsProfile` for the same effect without a second
   stickiness mechanism. Then the **Color Mixer**
   (`LabColor.applyColorMixer`, SwiftInvert-only, no NegPy equivalent —
   chroma-gated hue-targeted R/Y/G/B bands: per-band raised-cosine hue
   windows × a shared chroma ramp that zeroes at the neutral axis, so
   whites/grays/faint casts never move; all weights read the ORIGINAL hue
   and compose jointly, so overlapping feathers are order-independent;
   constants tuned on real scans (blues at Lab hue ~235, gate 6→16 —
   colorimetric-primary values starve real content, whose colorful pixels
   sit at chroma 12-30): `bandCentersDeg`/`bandHalfWidthsDeg`/`bandChromaGate*`/
   `bandMaxHueShiftDeg` mirrored as MSL literals; UI: segmented band
   picker + gradient tracks, `ColorMixerSection.swift`), then
   vibrance (muted-chroma boost, /60 range; upstream DELETED theirs —
   ours is SwiftInvert-maintained, reference formula frozen in the dump
   script) then saturation (a*,b* scale; **gamut-aware on boosts** since
   the 1b900ab port: skin-band softening (Gaussian around Lab hue 52°,
   width 25°, strength 0.5, chroma gate 2) → per-pixel in-gamut headroom
   by 10-iteration bisection against the real RGB cube → softplus knee,
   so overshooting pixels keep their hue instead of hue-shifting through
   the per-channel clamp; in-gamut non-skin pixels are bit-identical to
   the flat scale; desaturation stays flat; PURE gamut math since the
   bfcd90a port removed the in-boost skin term), then **Skin Protection**
   (`skinChromaRein`, default 0.5, slider 0–1): a one-directional soft
   chroma ceiling (22/strength, softplus knee from 0.6×ceiling, scale
   blended by mask weight; a*/b* together so hue and L* never move)
   inside the measured skin locus — hue Gaussian 52° σ20 × one-sided
   chroma window (full ≤35, zero ≥60; pure red C*≈104 drops out) ×
   lightness rolloff (L* 15…95) — applied after the scale and
   independent of it, so it also reins skin arriving over-chromatic
   from the print curve. All constants are MSL literals mirrored from
   LabColor.swift.
   Separate pass on purpose: inlining
   the Lab code into printCurve cost ~3 ms/frame in register pressure even
   when branched off. Writes into the (already consumed) `normalized`
   texture, which becomes the content texture.
4. **`histogram256`** — 4×256 atomic bins (R,G,B + Rec.709 luma) from the
   linear content, OETF-encoded in-shader so bins match display values —
   including the levels remap (below), so the interactive histogram
   reshapes live under a drag. Bins are RAW COUNTS, and the HQ tier bins
   ~15× as many pixels as the proxy, so anything drawn from them must be a
   function of the histogram's SHAPE only (`HistogramView.barHeight`
   normalizes to the peak bin, then compresses with a FIXED constant;
   `log1p(count)/log1p(maxCount)` looks scale-free but adds ln(pixelCount)
   to both ends, and lifted sparse bins 7–14 percentage points with HQ on).
   `clipFractions` and the hover read-out already divide by the total.
5. **`outputEncode`** — working-space OETF (Adobe RGB 1998 TRC: pure
   563/256 = 2.19921875 power, no linear segment — b3490eb), then the
   **levels remap** (interactive histogram, SwiftInvert-only): per channel
   a piecewise-linear map through user-planted anchors (input→output
   pairs, endpoints pinned, empty = identity; `levels_remap` MSL + the CPU
   mirror `ReferenceCurve.levelsRemap`; `sanitizeLevels` at derive sorts,
   clamps 0.02 off the endpoints, forces monotone outputs, caps at 8;
   uniforms travel as a flat 51-float buffer — layout documented at both
   ends). Display fast path writes the same kernel
   into an **rgba8unorm** texture (GPU quantization, 4× smaller readback,
   zero CPU conversion); export/tests use the float path.

**Threading/reuse contract**: `render` is serialized by an internal lock and
returns **read-back buffers, never live textures** — intermediate textures
are cached per size (≤4 MP; export sizes are not retained) and a later render
overwrites them. Violating this segfaulted the concurrent test runner once.
Uniform structs are mirrored byte-for-byte in `ShaderTypes.swift`;
`LayoutTests` pins strides (Norm 48, Curve 256) plus the 51-float levels-buffer length and key offsets — update both
sides plus the asserts together.

**Big-buffer discipline** (learned repeatedly, now applied throughout): never
allocate a large pixel buffer with `[T](repeating:)` when the code that
follows writes every lane — use `unsafeUninitializedCapacity` (or
`RGBImage(width:height:initializingWith:)`). At export size the wasted
zero-fill costs tens of ms per buffer in first-touch page faults alone, and
`readback` was doing it twice (387 MB + 290 MB). Live instances: `upload`,
`readback`, both display/float read-backs, `RawDecoder`'s u16→float,
`RGBImage.downsampled`, `BoundsAnalysis.sortedChannels`.

### 5. Color management

Working space is **linear Adobe RGB (1998) → Adobe-RGB-encoded** at the end
of the chain (b3490eb port, 2026-07-20: upstream moved off ProPhoto because
the pipeline ASSIGNS primaries to sensor-native data at output, and
ProPhoto's imaginary primaries inflated chroma and skewed hues — reds
toward magenta, skies toward cyan).
- **Display**: the encoded output becomes a CGImage tagged
  `CGColorSpace.adobeRGB1998`; ColorSync converts to the monitor — pinned
  against NegPy's littleCMS display transform (AdobeCompat-v4 → sRGB-v4,
  relative colorimetric + BPC) on reference colors (`ColorIOTests` oracle
  values regenerated 2026-07-20 with NegPy's bundled ICC profiles).
- **Export** (`Exporter` + `ColorIO`): default **sRGB** (NegPy's default;
  wide-gamut files look washed out in profile-ignorant viewers) via a 16-bit
  working-space CGImage drawn into an sRGB CGContext (one quantization).
  Adobe RGB remains selectable (the ExportColorSpace case is still NAMED
  rommRGB for sticky-options JSON compatibility). JPEG 8-bit (default q=0.92) or TIFF 16-bit, optional
  long-edge resize, destination next-to-source or a chosen folder.

## Parity with NegPy

`Tests/Fixtures/` is dumped from NegPy's actual engine by
`scripts/dump_fixtures.py` (run in NegPy's uv env). The suite (`make test`)
verifies every stage boundary:
- closed-form oracles (percentile semantics, grade/pivot/softplus/OETF math),
- `synthetic64` (NegPy's golden image, dumped as .bin): prefiltered grid
  @1e-5, bounds/meters/curve-params @1e-4, full CPU chain @1e-4,
- `synthetic_grid` (1600×1066, regenerated bit-exactly in Swift from an
  integer-hash formula): exercises b=2 block-median + the quadratic
  cast-removal path (slope/pivot tolerance 2.5e-4: Accelerate-vs-libm ulp
  amplification),
- `ramp257` print-curve shapes, `lab_color` (vibrance/saturation vs NegPy's
  CIELAB ops),
- GPU vs fixtures and GPU vs CPU reference at NegPy's own gates (mean<0.01,
  max<0.04); the rgba8 display path within 1.5/255 of the float path.

Beyond parity, the suite covers the seams the fixtures reach only
transitively, plus the app layer:
- `SidecarCodecTests` + `HistoryLabelTests` are **drift-catchers**: each pins `ExposureSettings`' stored-property count (52) and exercises every field —
  adding a settings field fails both until the decoder, `HistoryLabels`, and
  the tests' mutation lists all get their line (see the control checklist).
- `ImagePipelineSeamTests`: the prepare/finalize cache split (a reused
  `Prepared` must equal fresh analysis; offsets may only move the neutral
  axis) and `RGBImage.downsampled` (dims/identity/mean preservation).
- **`SwiftInvertTests`** — the app-target suite. SwiftPM tests the `@main`
  executable directly (`@testable import SwiftInvert` — works since Swift
  5.5, verified under the Makefile's CLT flags): `SidecarStore` file behavior
  (legacy `.negswift.json` fallback + delete-on-save),
  `ExportOptions.destinationURL`, `DensitometerState` probe mapping,
  `ImageConversion` shapes. Views/AppModel/ImageSession stay UI-verified —
  and pointer paths need a HUMAN pointer: synthetic mouse events never reach
  the unbundled binary (no TCC grant, won't take focus), so verify hover
  logic headlessly (`negcli meter` pattern) and hand off the gesture.

**Upstream review log: see `UPSTREAM.md`** — it records the last NegPy
commit reviewed (the baseline for "what changed upstream?" requests) and
the port/skip decisions per review. Update it after every upstream review.
For "what changed in NegPy?" requests, run the **`/negpy-review` skill**
(`.claude/skills/negpy-review/`) — it fetches upstream, triages the diff
around the inversion pipeline, and maintains UPSTREAM.md.

Analysis semantics and kernel constants are synced with **NegPy 0.43**
(`0369b10` tip; the estimator rewrite landed in `127bcd7`: two-pass
RMS-chroma neutral axis with the rebuilt confidence, same-pixel colour
floors, `neutral_axis_chroma_cap` 0.29 + the five new estimator constants —
on top of the earlier syncs: 2125a34 pre-trim neutral axis, b3490eb-coupled
auto constants, the 0.38 set with `paper_dmin` off + `true_black` on, the
0.36 set). Fixtures were re-dumped from `0369b10` (the manifest records
`paper_dmin` and `true_black` per config; the parity harnesses read both,
so default flips on either side can't silently skew parity). NegPy's per-layer R/G/B trims,
Split Grade and Zone Density (their convergent take on our tone controls)
are NOT ported — our tone controls + 3-band grading cover the achromatic
cases; per-channel crossover trims are a candidate future feature.

**Deliberate divergences from NegPy** (fixture tests pin the NegPy-neutral
values where needed):
- `preSaturation` default **1.15** (NegPy has no equivalent; parity tests set 1.0),
- ~~`redHue` default +0.5~~ RETIRED 2026-07-20: it countered reds skewing
  magenta, which was the ProPhoto-interpretation hue skew the b3490eb port
  removed at the root — default is 0 again (sidecars that explicitly set a
  redHue keep their value; keyless sidecars now decode to neutral),
- Color Mixer band constants were tuned in the old ROMM/D50 Lab; re-checked
  numerically after b3490eb (real-content reds/greens/sky-blues still land
  in-band; only colorimetric-primary blue sits at the feather edge, which
  matches the tuned-on-real-content philosophy) — but an on-scan re-tune
  pass is still worth doing,
- default analysis buffer **0.10** vs NegPy 0.05 (tests pass 0.05 explicitly),
- NegPy's default lab sharpen (0.25 since 8bc9678; was 0.5 earlier in 0.38) is not implemented,
- **vibrance is SwiftInvert-maintained since NegPy de79e13** (upstream
  deleted Lab Vibrance in favour of a density-space signed Dye Separation —
  which they then also deleted in 3fb5ca8, leaving only the matrix-slot
  control we ship as Print Saturation; ours stays for sidecar compat,
  identity in every parity config — the lab_color vibrance fixture is
  frozen at the 0369b10 dump and cannot be re-dumped from newer NegPy),
- SwiftInvert-only controls: exposure stops, tone controls
  (shadows/highlights ± contrasts), overall contrast, temp/tint, 3-band
  color grading, pre-saturation, the interactive-histogram levels remap —
  all identity-at-default so the NegPy fixtures still pass.

## App layer (`Sources/SwiftInvert`)

- **`AppModel`** (@MainActor @Observable): selection + `multiSelection`,
  settings (didSet → coalesced latest-wins render task + debounced 1 s
  sidecar save; `isRestoringSettings` suppresses saves on open), tool modes
  (crop/analysis-region draw), baseline hold-to-compare
  (`showingBaseline` renders stock settings with current geometry), export
  batch task with progress/cancel.
- **`ImageSession`** (actor): the per-image cache tower (see §2) + render.
- **Default profiles** (`DefaultProfile.swift`, `ProfileStore.swift`,
  `ProfileWindows.swift`): the house look applied wherever a frame is seen
  fresh (open with no sidecar, Reset All, sidecar-less frames in batch
  export and Copy Adjustments) — resolved as `DefaultProfile.settings` =
  the ACTIVE `SettingsProfile` in `ProfileStore` (two reserved rows from
  code, never persisted/editable: "None" = stock, first in the list and
  the out-of-the-box active choice, then "SwiftInvert Default" = the house
  look; user profiles + active choice as UserDefaults JSON, so a picked
  default survives restarts). File → Choose
  Default Settings… opens the picker (Accept = set active; Create New
  seeds from the selection); Create/Edit opens a **profile-editor window**:
  the full app UI under `AppModel(profileEditor: true)`, where one shared
  adjustments draft (`profileDraft`) applies to EVERY frame — geometry
  stays per-frame from sidecars, and NO sidecar is ever written in that
  mode (guards in scheduleSave/paste/export-flush). Multi-window plumbing:
  `KeyModelTracker` + `WindowKeyObserver` route menu commands and each
  ContentView's key monitor to the key window's model. "Start from
  Scratch" (under View Original) cancels the profile to stock
  `ExposureSettings()`, which stays NegPy-parity-neutral — fixtures,
  negcli and the parity suite never see profiles. `DefaultProfileTests` +
  `ProfileStoreTests` pin the built-in's field list, adjustments-only
  stripping, built-in protection, and persistence round-trip.
- **Interactive Histogram** (`InteractiveHistogram.swift`, double-click
  the sidebar histogram → its own window, key-model capture at open):
  per-channel levels remap by direct drag. A drag grabs a tone (the grab
  position is inverse-mapped through the remap in effect, so you grab
  what you SEE — pressing near an existing anchor's line grabs THAT
  anchor) and moves its output; releasing PLANTS a fixed anchor, and
  later drags reshape only their own segment between neighbours. Anchors
  render as vertical lines with an ✕ below to remove; outputs clamp
  between neighbours so the map stays monotone; ≤ 8 per channel
  (`levelsRed/Green/Blue` anchor arrays). Drags commit on release via
  setControlEditing; per-channel + all reset buttons.
- **Test strip** (NegPy e4bc450 port: `TestStrip` math in NegativeKit,
  `ImageSession.renderTestStrip` — one analysis + one uploaded source +
  25 derive/renderDisplay passes, mosaic assembled incrementally, always
  the preview proxy — `TestStripLayer` presentation, Print-group button +
  ⇧T in the key monitor): the frame as a 5×5 grid of REAL renders,
  Brightness across (mirrored from upstream's density order to match our
  right-=-brighter convention), Grade R75–R155 down, ladders absolute and
  centred on the defaults so the current settings are a patch. Two-stage
  picking: single-click a patch → full-frame PREVIEW of that look (one
  warm-tower render; click again = back to the grid to compare others),
  double-click (grid or preview) → one history entry ("Test strip pick")
  setting density+grade (auto toggles untouched — patches rendered under
  them); singles are held for the double-click interval on purpose (a
  mis-fired confirm would commit settings); session-only
  state cleared by ANY edit/navigation/tool/baseline/HQ change (guarded
  by a generation counter against stale builds); Escape clears first.
  No gridlines — only the hovered patch outlines; the current-settings
  rung is accented. While a strip is up the rotate buttons and ⌘[/⌘]
  turn the LADDER, not the image (a2455ab port): orientation 0–3 rotates
  the logical density×grade assignment under the fixed display grid,
  labels/picks follow, mosaic rebuilds from the warm tower (upstream
  caches all four assemblies; we re-render — same UX at ~a strip build,
  no 25-tile retention). TestStripRenderTests pins patch == full render
  at that patch's settings, byte-identical; rotation tests pin the
  bijection and axis purity per orientation.
- **Darkroom read-outs** (`Densitometry` in NegativeKit — pure measurement, no
  render path, so no parity surface): the **spot densitometer** (hover the
  canvas → D + zone in the control bar, with an 11-cell `ZoneStrip`) and the
  **Negative character** row under the Grade slider ("0.90 · normal"), which is
  the measured pre-offset density range vs `CurveLogic.defaultGradeRange`.
  Both mirror NegPy (`densitometer.py`, `stats._negative_row`), including its
  quirk of taking luma on the ENCODED triplet before decoding. The probe reads
  the **displayed rgba8 bitmap** (working-space-encoded, so the bytes already are the
  working-space values) rather than a GPU metric — `DensitometerState` caches
  the provider bytes once per render and is a separate `@Observable` so pointer
  moves invalidate only the read-out label, never the canvas. The **zone
  overlay** (NegPy ae56f8b port: `ZoneGrid` in NegativeKit + `ZoneOverlay`
  view; "Zones" control-bar button / View menu / ⇧Z) shares that ruler AND
  those cached bytes: integer zone per cell (24 along the long edge,
  box-averaged), same-zone regions merged with one Roman numeral each
  (anchor snapped to an owned cell), paper black/white numerals red. The
  grid recomputes per adopt/toggle only, and hides in tool modes and
  straighten previews (same rule as the probe). ⇧Z lives in the window key
  monitor, NOT as a menu equivalent — a bare-letter equivalent would fire
  while typing in text fields; the monitor checks the first responder. NegPy's 120-bin
  `density_histogram` is deliberately NOT ported: it exists to feed their H&D
  chart, which we don't ship.
- **Zone placement** (NegPy 5a095f3/9dff124 port: `ZonePlacement` in
  NegativeKit + `ZonePlacementLayer`/AppModel state): click a zone-strip
  cell, click that spot on the photo, and Brightness (1 pin) or
  Brightness+Grade (2 pins) or those plus ONE knee control (3 pins) solve
  so the tone prints there. Pins freeze their normalized-log luma
  (`ImageSession.sampleZonePin`, 5×5 patch mean — the displayed rgba8 is
  post-curve and can't serve); the forward model is the ACHROMATIC
  green-reference curve run through the real kernel (`predictedZone`:
  derive → params collapsed to green → 1-px `ReferenceCurve` → zone
  ruler — includes exposureStops and the green levels remap, which
  upstream's model omits/lacks). Solved by bisection at half slider
  precision, autos forced off, clamped+rounded, achieved zones recomputed
  at the rounded values; `encoded(ofZone:)` is the ruler's exact inverse.
  Knee candidates are OUR tone-control knees (shadowContrast /
  highlightContrast — upstream solves Split Grade/Snap, which we don't
  ship), picked by measured purchase; DIVERGENCE: the knee bisects the
  COMPOSITE residual (2-pin solve nested inside) instead of upstream's
  alternating passes — our anchors sit between typical pin positions, the
  alternation diverges there, and the µs-scale model makes nesting cheap.
  UX mirrors the test strip's canvas-state rules: solved-look preview
  drawn over the frame, drag re-samples (retargeted pins keep their
  zone), Apply = one history entry ("Zone placement"), Escape/any
  edit/tool/baseline/HQ/navigation clears; solve runs in the session
  actor. No settings field, no parity surface, no fixture re-dump.
- **Edit history**: per-image undo/redo in AppModel (`historyEntries`/`historyIndex`,
  session-scoped per URL). Slider/handle drags commit on RELEASE (drag =
  preview: `setControlEditing` via the `controlEditingChanged` environment
  hook holds commits while any control is held); non-drag changes coalesce
  via a 0.7 s debounced commit;
  labels come from `historyLabel(from:to:)` (HistoryLabels.swift — add a line
  there for every new settings field) or `pendingHistoryLabel` for named
  actions (Rotate/Crop/Reset). New edits truncate the redo tail. Undo flushes
  any in-flight uncommitted change first. UI: HistoryPanel (⌘Z/⇧⌘Z,
  click-to-jump), below the collapsible Adjustments section.
- **Unified Crop & Straighten** (Lightroom model): `ToolMode.crop` renders
  the full UNROTATED frame (scheduleRender substitutes fineRotation 0,
  uncropped) fitted by its rotated bounding box; the image rotates behind an
  axis-aligned `CropBoxOverlay` (dim surround, thirds, corner handles,
  clamped move) whose math lives in `NegativeKit/CropGeometry` (rotated-space
  boxes, fitScale/constrain, content-preserving `remapCrop`; tested). The
  desired box is DetailView `@State` (`cropBox`, nil = follow committed);
  exit commits via `commitCrop()` (near-full box ⇒ crop cleared). Straighten
  commits outside the mode go through `AppModel.commitFineRotation`, which
  remaps a committed crop so its content doesn't drift with the inscribed
  auto-crop; `RenderOutput.frameSize` (orientation-only dims) is the
  coordinate base. The analysis tool keeps the old draw-a-rect
  `SelectionOverlay`.
- **Menu bar** (`SwiftInvertApp` `.commands`): File = Open Folder ⌘O /
  Choose Default Settings… / Export ⌘E / Show in Finder ⇧⌘R; Edit = Undo/Redo (replacing the system
  group — the ⌘Z shortcuts live HERE, not on HistoryPanel's buttons),
  Copy/Paste Adjustments ⇧⌘C/⇧⌘V (geometry never pasted), Reset All ⌥⌘R;
  View = Show Library ⇧⌘L / Show Grid Lines ⇧⌘G / Zone Overlay (⇧Z via the
  key monitor) / HQ Preview (inline 3-way picker — a Picker can't carry a key
  equivalent, so a sibling "Cycle HQ Preview" item owns ⇧⌘P and cycles like
  the badge)
  (@AppStorage keys shared with the in-window controls); Image =
  Previous/Next Image ←/→ / Rotate Left/Right ⌘[/⌘] / Flip ⇧⌘H /
  Crop ⌘K / Crop for Analysis ⇧⌘K + clear items (tool toggles checkmark
  while active; Escape exits). Frame navigation also answers ↑/↓ (↑ =
  previous, ↓ = next) via the window's keyDown monitor in
  `SwiftInvertApp` — a menu item takes exactly one key equivalent, so the
  vertical pair can't live on the menu; the monitor mirrors the menu
  items' enablement (no files / export sheet up ⇒ pass through).
- **Sidecars**: `<basename>.swiftinvert.json` next to the source
  (`SidecarStore`); pre-rename `.negswift.json` read as fallback and removed
  on next save. Missing keys decode to defaults (custom `init(from:)` in
  `ExposureSettings` — every new field needs a line there).
- **UserDefaults**: `libraryFolder`, `canvasColor`, `exportOptions`
  (JSON blob); one-time migration from the pre-rename "NegSwift" domain.
- **SwiftUI pitfalls already hit** (don't regress):
  - Lazy grids + @Observable: cells must read observable state inside their
    own body — precomputed Bools leave stale cells (selection halo bug).
  - `.position()` expands a view's frame to its container: attach
    `contentShape`/gestures BEFORE positioning (histogram handle bug).
  - `NSEvent.modifierFlags` polling in tap handlers is unreliable — use
    `TapGesture().modifiers(.command)` etc.
  - The app runs unbundled via `swift run`: activation policy is set
    manually; UserDefaults/TCC key off the process name.

## Adding a new adjustment control (checklist)

1. `ExposureSettings`: field + default, **plus a line in the custom
   decoder** (sidecar back-compat), **plus `historyLabel`**
   (HistoryLabels.swift) — and `RenderParams` + its init if the kernel
   needs it. Two tripwires enforce this: `SidecarCodecTests` and
   `HistoryLabelTests` both pin the stored-property count (52) and mutate
   every field, so `make test` fails until all the lists have their line.
2. `deriveRenderParams`: map settings → params (fold into existing params
   where the algebra allows — see overall contrast/exposure — before adding
   uniforms).
3. Kernels, BOTH sides in the same change: `ReferenceCurve.swift` (CPU) and
   `NegPipeline.metal` (MSL), in identical order/parallel form. New uniforms:
   extend `CurveUniforms` in both files (16-byte alignment) + update
   `LayoutTests` strides/offsets. Guard non-default-off work behind uniform
   branches or a separate pass (occupancy!).
4. UI: `LabeledSlider`/`GradientSlider` (sets the reset-⨯ default), section
   in `ControlsSidebar`; `negcli render` flag if useful.
5. Tests: default-is-identity (fixtures must still pass), direction/region
   properties, monotonicity if it touches the tone curve, sidecar
   round-trip, GPU-parity case with the control active.
6. Run `make test`, `negcli bench` (watch for regressions), launch the app
   headlessly (render log line), commit with the reasoning.

## Constants sync points

- `K` (`ExposureConstants.swift`) is the single Swift source; the MSL
  duplicates: tone anchors/sharpness (`TONE_SHARPNESS`, `SHADOW_ANCHOR`,
  `HIGHLIGHT_ANCHOR`), Lab matrices/eps/kappa/white, and the working-space
  OETF exponent (0.45470693 in MSL = 256/563).
  GPU/CPU parity tests catch drift but update them together.
- If NegPy's `EXPOSURE_CONSTANTS` change deliberately: update `K`, re-dump
  fixtures, re-run `make test`.
